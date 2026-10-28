use libloading::Library;
use std::{
    ffi::{c_char, c_int, c_void},
    io::{Read, Write},
    marker::PhantomData,
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicI32, Ordering},
    },
    thread,
    time::Duration,
};
use thiserror::Error;
use tokio::sync::oneshot;
use zeroize::Zeroizing;

#[derive(Debug, Error)]
pub enum Error {
    #[error("A biblioteca P2P Linux deve ser um arquivo com caminho absoluto.")]
    InvalidLibrary,
    #[error("A biblioteca P2P Linux não corresponde à versão de ABI validada.")]
    UnsupportedLibrary,
    #[error("A biblioteca P2P Linux não contém todos os símbolos necessários.")]
    MissingSymbol,
    #[error("Parâmetro P2P inválido.")]
    InvalidInput,
    #[error("Não foi possível criar o túnel P2P Linux (código {0}).")]
    Open(c_int),
    #[error("O túnel P2P Linux falhou (status {0}).")]
    Status(c_int),
    #[error("Tempo limite aguardando o túnel P2P Linux.")]
    Timeout,
    #[error("Não foi possível restringir o listener P2P ao computador local.")]
    Isolation,
}

#[repr(C)]
#[derive(Default)]
struct Report {
    fields: [u32; 17],
}
type Open = unsafe extern "C" fn(
    *const c_char,
    u16,
    *const c_char,
    *const c_char,
    *const c_char,
    *const c_char,
    u16,
    u32,
    *mut *mut c_void,
    *mut Report,
) -> c_int;
type Transfer = unsafe extern "C" fn(*mut c_void, *mut c_void, u32, u32) -> c_int;
type SendBytes = unsafe extern "C" fn(*mut c_void, *const c_void, u32, u32) -> c_int;
type Close = unsafe extern "C" fn(*mut c_void);

struct Api {
    _library: Library,
    open: Open,
    read: Transfer,
    write: SendBytes,
    close: Close,
}
struct Service {
    server: Zeroizing<Vec<u8>>,
    key: Zeroizing<Vec<u8>>,
    port: u16,
}
struct Destination {
    service: Arc<Service>,
    serial: Zeroizing<Vec<u8>>,
    password: Zeroizing<Vec<u8>>,
    ip: Zeroizing<Vec<u8>>,
    port: u16,
    timeout: u32,
}

pub struct Client {
    api: Arc<Api>,
    service: Arc<Service>,
    workers: Mutex<Vec<thread::JoinHandle<()>>>,
}
impl Client {
    pub fn load(path: &Path, server: &str, port: u16, key: &str) -> Result<Self, Error> {
        if !path.is_absolute() || !path.is_file() {
            return Err(Error::InvalidLibrary);
        }
        if server.is_empty() || port == 0 || key.is_empty() {
            return Err(Error::InvalidInput);
        }
        let service = Arc::new(Service {
            server: terminated(server)?,
            key: terminated(key)?,
            port,
        });
        // SAFETY: biblioteca local explicitamente selecionada. A versão é validada
        // antes de carregar as assinaturas do header sim_t2u.h. Arc retém a Library
        // até que o último worker libere seu handle nativo.
        let api = unsafe {
            let library = Library::new(path).map_err(|_| Error::InvalidLibrary)?;
            let version = library
                .get::<unsafe extern "C" fn() -> u32>(b"sim_t2u_abi_version\0")
                .map_err(|_| Error::MissingSymbol)?;
            if version() != 1 {
                return Err(Error::UnsupportedLibrary);
            }
            Api {
                open: *library
                    .get(b"sim_t2u_open\0")
                    .map_err(|_| Error::MissingSymbol)?,
                read: *library
                    .get(b"sim_t2u_read\0")
                    .map_err(|_| Error::MissingSymbol)?,
                write: *library
                    .get(b"sim_t2u_write\0")
                    .map_err(|_| Error::MissingSymbol)?,
                close: *library
                    .get(b"sim_t2u_close\0")
                    .map_err(|_| Error::MissingSymbol)?,
                _library: library,
            }
        };
        Ok(Self {
            api: Arc::new(api),
            service,
            workers: Mutex::new(Vec::new()),
        })
    }

    pub async fn tunnel(
        &self,
        serial: &str,
        tunnel_password: &str,
        remote_ip: &str,
        remote_port: u16,
        timeout: Duration,
    ) -> Result<Tunnel<'_>, Error> {
        if serial.is_empty() || remote_port == 0 || remote_ip.is_empty() {
            return Err(Error::InvalidInput);
        }
        let timeout = u32::try_from(timeout.as_millis())
            .ok()
            .filter(|n| (1..=120000).contains(n))
            .ok_or(Error::InvalidInput)?;
        let destination = Arc::new(Destination {
            service: self.service.clone(),
            serial: terminated(&serial.to_ascii_uppercase())?,
            password: terminated(tunnel_password)?,
            ip: terminated(remote_ip)?,
            port: remote_port,
            timeout,
        });
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|_| Error::Isolation)?;
        listener
            .set_nonblocking(true)
            .map_err(|_| Error::Isolation)?;
        let endpoint = listener.local_addr().map_err(|_| Error::Isolation)?;
        let state = Arc::new(State {
            stop: AtomicBool::new(false),
            status: AtomicI32::new(0),
        });
        let tunnel = Tunnel {
            endpoint,
            state: state.clone(),
            _client: PhantomData,
        };
        let api = self.api.clone();
        let (ready, opened) = oneshot::channel();
        let worker = thread::Builder::new()
            .name("simneo-p2p-listener".into())
            .spawn(move || {
                let initial = match Channel::open(api.clone(), &destination) {
                    Ok(channel) => channel,
                    Err(error) => {
                        let _ = ready.send(Err(error));
                        return;
                    }
                };
                state.status.store(1, Ordering::Release);
                if ready.send(Ok(())).is_err() {
                    return;
                }
                if listen(listener, api, destination, &state, initial).is_err() {
                    state.status.store(-3, Ordering::Release);
                }
                state.stop.store(true, Ordering::Release);
            })
            .map_err(|_| Error::Open(-3))?;
        let mut workers = self.workers.lock().map_err(|_| Error::Open(-3))?;
        let mut index = 0;
        while index < workers.len() {
            if workers[index].is_finished() {
                let _ = workers.swap_remove(index).join();
            } else {
                index += 1;
            }
        }
        workers.push(worker);
        drop(workers);
        // Se a future for cancelada, Drop sinaliza o worker. A abertura nativa
        // termina no timeout definido; nenhum handle ou biblioteca é liberado em uso.
        opened.await.map_err(|_| Error::Open(-3))??;
        Ok(tunnel)
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // Todos os túneis já foram destruídos (empréstimo de Client). Aguardar os
        // workers garante o fechamento VDT antes da saída do processo e do dlclose.
        if let Ok(workers) = self.workers.get_mut() {
            for worker in workers.drain(..) {
                let _ = worker.join();
            }
        }
    }
}

struct State {
    stop: AtomicBool,
    status: AtomicI32,
}
pub struct Tunnel<'a> {
    endpoint: SocketAddr,
    state: Arc<State>,
    _client: PhantomData<&'a Client>,
}
impl Tunnel<'_> {
    pub fn endpoint(&self) -> SocketAddr {
        self.endpoint
    }
    pub fn status(&self) -> Result<c_int, Error> {
        let code = self.state.status.load(Ordering::Acquire);
        if code < 0 {
            Err(Error::Status(code))
        } else {
            Ok(code)
        }
    }
}
impl Drop for Tunnel<'_> {
    fn drop(&mut self) {
        self.state.stop.store(true, Ordering::Release);
    }
}

struct Channel {
    handle: *mut c_void,
    api: Arc<Api>,
}
// SAFETY: o C mantém apenas socket e buffers próprios por handle, sem TLS.
// Mover transfere a posse exclusiva; Channel não é Sync e as chamadas continuam
// serializadas no worker. Api retém a biblioteca durante toda a transferência.
unsafe impl Send for Channel {}
impl Channel {
    fn open(api: Arc<Api>, to: &Destination) -> Result<Self, Error> {
        let mut handle = std::ptr::null_mut();
        let mut report = Report::default();
        // SAFETY: buffers NUL terminados e vivos durante a chamada síncrona;
        // cada handle pertence a apenas um worker, sem chamadas concorrentes.
        let code = unsafe {
            (api.open)(
                to.service.server.as_ptr().cast(),
                to.service.port,
                to.service.key.as_ptr().cast(),
                to.serial.as_ptr().cast(),
                to.password.as_ptr().cast(),
                to.ip.as_ptr().cast(),
                to.port,
                to.timeout,
                &mut handle,
                &mut report,
            )
        };
        if code != 0 || handle.is_null() {
            if !handle.is_null() {
                unsafe {
                    (api.close)(handle);
                }
            }
            return Err(if code == -4 {
                Error::Timeout
            } else {
                Error::Open(if code == 0 { -6 } else { code })
            });
        }
        Ok(Self { handle, api })
    }
    fn read(&mut self, buffer: &mut [u8]) -> Result<usize, Error> {
        let code = unsafe {
            (self.api.read)(
                self.handle,
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
                10,
            )
        };
        match code {
            -4 => Ok(0),
            n if n > 0 && n as usize <= buffer.len() => Ok(n as usize),
            n => Err(Error::Status(n)),
        }
    }
    fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let code = unsafe {
            (self.api.write)(self.handle, bytes.as_ptr().cast(), bytes.len() as u32, 2000)
        };
        if code == bytes.len() as c_int {
            Ok(())
        } else {
            Err(Error::Status(code))
        }
    }
}
impl Drop for Channel {
    fn drop(&mut self) {
        unsafe {
            (self.api.close)(self.handle);
        }
    }
}

fn listen(
    listener: TcpListener,
    api: Arc<Api>,
    destination: Arc<Destination>,
    state: &Arc<State>,
    initial: Channel,
) -> Result<(), Error> {
    let mut initial = Some(initial);
    let mut pending = Vec::new();
    let mut workers = Streams {
        state: state.clone(),
        handles: Vec::new(),
    };
    while !state.stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => {
                let first = initial.take();
                let api = api.clone();
                let destination = destination.clone();
                let state = state.clone();
                let pending = std::mem::take(&mut pending);
                workers.handles.push(
                    thread::Builder::new()
                        .name("simneo-p2p-stream".into())
                        .spawn(move || {
                            let channel = first
                                .map(Ok)
                                .unwrap_or_else(|| Channel::open(api, &destination));
                            match channel {
                                Ok(mut channel) => {
                                    if let Err(error) =
                                        forward(stream, &mut channel, &state, pending)
                                    {
                                        eprintln!(
                                            "P2P Linux, canal TCP {}: {error}",
                                            destination.port
                                        );
                                    }
                                }
                                Err(error) => eprintln!(
                                    "P2P Linux, abertura TCP {}: {error}",
                                    destination.port
                                ),
                            }
                        })
                        .map_err(|_| Error::Status(-3))?,
                );
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if let Some(channel) = initial.as_mut() {
                    let mut buffer = [0; 65536];
                    let count = channel.read(&mut buffer)?;
                    if pending.len() + count > 65536 {
                        return Err(Error::Status(-6));
                    }
                    pending.extend_from_slice(&buffer[..count]);
                } else {
                    thread::sleep(Duration::from_millis(10));
                }
            }
            Err(_) => return Err(Error::Status(-3)),
        }
        let mut index = 0;
        while index < workers.handles.len() {
            if workers.handles[index].is_finished() {
                if workers.handles.swap_remove(index).join().is_err() {
                    return Err(Error::Status(-3));
                }
            } else {
                index += 1;
            }
        }
    }
    Ok(())
}

struct Streams {
    state: Arc<State>,
    handles: Vec<thread::JoinHandle<()>>,
}
impl Drop for Streams {
    fn drop(&mut self) {
        self.state.stop.store(true, Ordering::Release);
        for worker in self.handles.drain(..) {
            let _ = worker.join();
        }
    }
}

fn forward(
    mut stream: TcpStream,
    channel: &mut Channel,
    state: &State,
    mut incoming: Vec<u8>,
) -> Result<(), Error> {
    stream
        .set_nonblocking(true)
        .map_err(|_| Error::Status(-3))?;
    stream.set_nodelay(true).map_err(|_| Error::Status(-3))?;
    let mut buffer = [0; 65536];
    let mut offset = 0;
    while !state.stop.load(Ordering::Acquire) {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => channel.write(&buffer[..count])?,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => break,
        }
        if offset < incoming.len() {
            match stream.write(&incoming[offset..]) {
                Ok(0) => break,
                Ok(count) => offset += count,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(2));
                }
                Err(_) => break,
            }
        } else {
            incoming.clear();
            offset = 0;
            let count = channel.read(&mut buffer)?;
            incoming.extend_from_slice(&buffer[..count]);
        }
    }
    Ok(())
}

fn terminated(value: &str) -> Result<Zeroizing<Vec<u8>>, Error> {
    if value.as_bytes().contains(&0) {
        return Err(Error::InvalidInput);
    }
    let mut bytes = Zeroizing::new(value.as_bytes().to_vec());
    bytes.push(0);
    Ok(bytes)
}
