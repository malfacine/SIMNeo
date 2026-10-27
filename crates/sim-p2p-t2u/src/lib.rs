//! Wrapper do SDK P2P local; sem redistribuição da DLL proprietária.

#[cfg(windows)]
mod loopback;

use libloading::Library;
use sha2::{Digest, Sha256};
use std::{
    ffi::{c_char, c_int},
    marker::PhantomData,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::Path,
    rc::Rc,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use thiserror::Error;
use zeroize::Zeroizing;

static INITIALIZED: AtomicBool = AtomicBool::new(false);
const DLL_SHA256: &str = "528b8289c6f59e447c91508790d21abe888ad16ee1b1f6e30a52c4799bbf4d02";

#[repr(C)]
pub struct T2uNetStat {
    pub ip: [c_char; 20],
    pub port: c_int,
    pub proxy: c_int,
    pub lost_rate: f32,
    pub bandwidth: c_int,
    pub remote_nattype: c_int,
    pub local_nattype: c_int,
    pub ip6: [c_char; 40],
    pub remote_version: c_int,
}
impl Default for T2uNetStat {
    fn default() -> Self {
        Self {
            ip: [0; 20],
            port: 0,
            proxy: 0,
            lost_rate: 0.0,
            bandwidth: 0,
            remote_nattype: 0,
            local_nattype: 0,
            ip6: [0; 40],
            remote_version: 0,
        }
    }
}

#[repr(C)]
struct Callbacks {
    connect: extern "C" fn(u16, c_int),
    disconnect: extern "C" fn(u16, c_int),
}
extern "C" fn ignore_callback(_: u16, _: c_int) {}
static CALLBACKS: Callbacks = Callbacks {
    connect: ignore_callback,
    disconnect: ignore_callback,
};

type Init = unsafe extern "C" fn(*const c_char, u16, *const c_char);
type SetCallback = unsafe extern "C" fn(*const Callbacks);
type Add = unsafe extern "C" fn(*const c_char, *const c_char, *const c_char, u16, u16) -> c_int;
type Status = unsafe extern "C" fn(u16, *mut T2uNetStat) -> c_int;
type Delete = unsafe extern "C" fn(u16);
type Exit = unsafe extern "C" fn();

#[derive(Debug, Error)]
pub enum Error {
    #[error("O SDK P2P já está inicializado neste processo.")]
    AlreadyInitialized,
    #[error("A DLL P2P deve ser um arquivo de instalação conhecido com caminho absoluto.")]
    InvalidLibrary,
    #[error("A DLL P2P não corresponde à versão de ABI validada.")]
    UnsupportedLibrary,
    #[error("A DLL P2P não contém todos os símbolos necessários.")]
    MissingSymbol,
    #[error("Parâmetro P2P inválido.")]
    InvalidInput,
    #[error("Não foi possível criar o túnel P2P (código {0}).")]
    Open(c_int),
    #[error("O túnel P2P falhou (status {0}).")]
    Status(c_int),
    #[error("Tempo limite aguardando o túnel P2P.")]
    Timeout,
    #[error("Não foi possível restringir o listener P2P ao computador local.")]
    Isolation,
}

struct Exclusive;
impl Drop for Exclusive {
    fn drop(&mut self) {
        INITIALIZED.store(false, Ordering::Release);
    }
}

/// Uso restrito à thread que inicializou a biblioteca global. Não é Send/Sync.
pub struct Client {
    #[cfg(windows)]
    _bind_guard: loopback::BindGuard,
    _library: Library,
    _exclusive: Exclusive,
    _thread_bound: PhantomData<Rc<()>>,
    _server: Zeroizing<Vec<u8>>,
    _key: Zeroizing<Vec<u8>>,
    add: Add,
    status: Status,
    delete: Delete,
    exit: Exit,
}

impl Client {
    pub fn load(path: &Path, server: &str, port: u16, key: &str) -> Result<Self, Error> {
        if !path.is_absolute() || !path.is_file() {
            return Err(Error::InvalidLibrary);
        }
        let bytes = std::fs::read(path).map_err(|_| Error::InvalidLibrary)?;
        let hash: String = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if hash != DLL_SHA256 {
            return Err(Error::UnsupportedLibrary);
        }
        if port == 0 || server.is_empty() {
            return Err(Error::InvalidInput);
        }
        let server = terminated(server)?;
        let key = terminated(key)?;
        INITIALIZED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| Error::AlreadyInitialized)?;
        let exclusive = Exclusive;
        // SAFETY: caminho explícito e hash da DLL x64 inspecionada. Assinaturas
        // conferidas no wrapper embarcado. Library vive até depois de t2u_exit.
        unsafe {
            let library = Library::new(path).map_err(|_| Error::InvalidLibrary)?;
            let init: Init = *library
                .get(b"t2u_init\0")
                .map_err(|_| Error::MissingSymbol)?;
            let callback: SetCallback = *library
                .get(b"t2u_set_callback\0")
                .map_err(|_| Error::MissingSymbol)?;
            let add: Add = *library
                .get(b"t2u_add_port_v3\0")
                .map_err(|_| Error::MissingSymbol)?;
            let status: Status = *library
                .get(b"t2u_port_status\0")
                .map_err(|_| Error::MissingSymbol)?;
            let delete: Delete = *library
                .get(b"t2u_del_port\0")
                .map_err(|_| Error::MissingSymbol)?;
            let exit: Exit = *library
                .get(b"t2u_exit\0")
                .map_err(|_| Error::MissingSymbol)?;
            #[cfg(windows)]
            let bind_guard = loopback::BindGuard::install(init as *const ())?;
            init(server.as_ptr().cast(), port, key.as_ptr().cast());
            callback(&CALLBACKS);
            Ok(Self {
                #[cfg(windows)]
                _bind_guard: bind_guard,
                _library: library,
                _exclusive: exclusive,
                _thread_bound: PhantomData,
                _server: server,
                _key: key,
                add,
                status,
                delete,
                exit,
            })
        }
    }

    pub async fn tunnel(
        &self,
        serial: &str,
        tunnel_password: &str,
        remote_ip: &str,
        remote_port: u16,
        timeout: Duration,
    ) -> Result<Tunnel<'_>, Error> {
        if serial.is_empty() || remote_port == 0 {
            return Err(Error::InvalidInput);
        }
        let serial = terminated(&serial.to_ascii_uppercase())?;
        let password = terminated(tunnel_password)?;
        let remote_ip = terminated(remote_ip)?;
        #[cfg(windows)]
        let scope = loopback::ForwardScope::new();
        // SAFETY: buffers terminados em NUL retidos até o fechamento do túnel.
        // Porta local 0 solicita alocação ao próprio SDK, sem corrida bind/close.
        let port = unsafe {
            (self.add)(
                serial.as_ptr().cast(),
                password.as_ptr().cast(),
                remote_ip.as_ptr().cast(),
                remote_port,
                0,
            )
        };
        let local_port = u16::try_from(port)
            .ok()
            .filter(|p| *p != 0)
            .ok_or(Error::Open(port))?;
        #[cfg(windows)]
        if !scope.verified(local_port) {
            unsafe {
                (self.delete)(local_port);
            }
            return Err(Error::Isolation);
        }
        #[cfg(windows)]
        drop(scope);
        let tunnel = Tunnel {
            client: self,
            port: local_port,
            _serial: serial,
            _password: password,
            _remote_ip: remote_ip,
        };
        let started = Instant::now();
        loop {
            match tunnel.status()? {
                1 => return Ok(tunnel),
                0 => {}
                code => return Err(Error::Status(code)),
            }
            if started.elapsed() >= timeout {
                return Err(Error::Timeout);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // SAFETY: todos os Tunnel emprestam Client e já foram destruídos.
        unsafe {
            (self.exit)();
        }
    }
}

pub struct Tunnel<'a> {
    client: &'a Client,
    port: u16,
    _serial: Zeroizing<Vec<u8>>,
    _password: Zeroizing<Vec<u8>>,
    _remote_ip: Zeroizing<Vec<u8>>,
}
impl Tunnel<'_> {
    pub fn endpoint(&self) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), self.port)
    }
    pub fn status(&self) -> Result<c_int, Error> {
        let mut status = T2uNetStat::default();
        // SAFETY: buffer de 88 bytes, layout C conferido por teste de ABI.
        let code = unsafe { (self.client.status)(self.port, &mut status) };
        if code < 0 {
            Err(Error::Status(code))
        } else {
            Ok(code)
        }
    }
}
impl Drop for Tunnel<'_> {
    fn drop(&mut self) {
        // SAFETY: a porta pertence a este objeto e o SDK ainda está carregado.
        unsafe {
            (self.client.delete)(self.port);
        }
        #[cfg(windows)]
        loopback::release(self.port);
    }
}

fn terminated(value: &str) -> Result<Zeroizing<Vec<u8>>, Error> {
    if value.as_bytes().contains(&0) {
        return Err(Error::InvalidInput);
    }
    let mut bytes = Zeroizing::new(value.as_bytes().to_vec());
    bytes.push(0);
    Ok(bytes)
}
