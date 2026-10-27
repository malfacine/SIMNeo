//! Compatibilidade restrita ao SHA-256 validado em lib.rs. Redireciona somente
//! a importação bind da libt2u, durante criação do túnel ou rebind da sua porta.
//! Não modifica instruções executáveis, outras DLLs, arquivo em disco ou firewall.
use crate::Error;
use std::{
    cell::Cell,
    collections::BTreeSet,
    ffi::c_void,
    mem::size_of,
    ptr,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use windows_sys::Win32::{
    Networking::WinSock::{
        AF_INET, SO_TYPE, SOCK_STREAM, SOCKADDR, SOCKADDR_IN, SOCKET, SOCKET_ERROR, SOL_SOCKET,
        bind, getsockname, getsockopt,
    },
    System::{
        LibraryLoader::{
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            GetModuleHandleExW,
        },
        Memory::{PAGE_READWRITE, VirtualProtect},
    },
};

// dumpbin /imports: WS2_32.dll ordinal 2 (bind), IAT 0x402e0 + 11*8.
// Este offset só é usado após conferência do hash integral da DLL no chamador.
const BIND_IAT_RVA: usize = 0x40338;
static TRACKED_PORTS: Mutex<BTreeSet<u16>> = Mutex::new(BTreeSet::new());
thread_local! {
    static CREATING:Cell<bool>=const {Cell::new(false)};
    static VERIFIED_PORT:Cell<u16>=const {Cell::new(0)};
}

pub struct ForwardScope;
impl ForwardScope {
    pub fn new() -> Self {
        CREATING.set(true);
        VERIFIED_PORT.set(0);
        Self
    }
    pub fn verified(&self, port: u16) -> bool {
        VERIFIED_PORT.get() == port
    }
}
impl Drop for ForwardScope {
    fn drop(&mut self) {
        CREATING.set(false);
    }
}
pub fn release(port: u16) {
    if let Ok(mut ports) = TRACKED_PORTS.lock() {
        ports.remove(&port);
    }
}

pub struct BindGuard {
    slot: *mut AtomicUsize,
}
impl BindGuard {
    /// SAFETY: symbol pertence à DLL cujo hash e ABI foram conferidos, retida
    /// pelo Client. Instalação ocorre antes de iniciar qualquer thread do SDK.
    pub unsafe fn install(symbol: *const ()) -> Result<Self, Error> {
        let mut module = ptr::null_mut();
        unsafe {
            if GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
                    | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                symbol.cast(),
                &mut module,
            ) == 0
            {
                return Err(Error::Isolation);
            }
            let slot = module.cast::<u8>().add(BIND_IAT_RVA).cast::<AtomicUsize>();
            if !(slot as usize).is_multiple_of(size_of::<usize>())
                || (*slot).load(Ordering::Acquire) != bind as *const () as usize
            {
                return Err(Error::Isolation);
            }
            replace(
                slot,
                bind as *const () as usize,
                local_bind as *const () as usize,
            )?;
            Ok(Self { slot })
        }
    }
}
impl Drop for BindGuard {
    fn drop(&mut self) {
        // Client chama t2u_exit (encerra threads) antes de destruir este guard,
        // e somente depois descarrega a Library que contém a IAT.
        unsafe {
            let _ = replace(
                self.slot,
                local_bind as *const () as usize,
                bind as *const () as usize,
            );
        }
        if let Ok(mut ports) = TRACKED_PORTS.lock() {
            ports.clear();
        }
    }
}

unsafe fn replace(slot: *mut AtomicUsize, expected: usize, value: usize) -> Result<(), Error> {
    let mut previous = 0;
    unsafe {
        if VirtualProtect(
            slot.cast::<c_void>(),
            size_of::<usize>(),
            PAGE_READWRITE,
            &mut previous,
        ) == 0
        {
            return Err(Error::Isolation);
        }
        let exchanged = (*slot)
            .compare_exchange(expected, value, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
        let mut ignored = 0;
        let restored = VirtualProtect(
            slot.cast::<c_void>(),
            size_of::<usize>(),
            previous,
            &mut ignored,
        ) != 0;
        if !exchanged || !restored {
            return Err(Error::Isolation);
        }
    }
    Ok(())
}

unsafe extern "system" fn local_bind(socket: SOCKET, address: *const SOCKADDR, length: i32) -> i32 {
    // SAFETY: parâmetros vêm do bind da DLL validada; a estrutura IPv4 só é
    // lida quando ponteiro, família e tamanho são compatíveis. Sem unwind em FFI.
    unsafe {
        if address.is_null()
            || length < size_of::<SOCKADDR_IN>() as i32
            || (*address).sa_family != AF_INET
        {
            return bind(socket, address, length);
        }
        let mut ipv4 = ptr::read_unaligned(address.cast::<SOCKADDR_IN>());
        let port = u16::from_be(ipv4.sin_port);
        let creating = CREATING.try_with(Cell::get).unwrap_or(false);
        let tracked = TRACKED_PORTS
            .lock()
            .map(|ports| ports.contains(&port))
            .unwrap_or(false);
        if !creating && (port == 0 || !tracked) {
            return bind(socket, address, length);
        }
        let mut kind = 0i32;
        let mut kind_len = size_of::<i32>() as i32;
        if getsockopt(
            socket,
            SOL_SOCKET,
            SO_TYPE,
            (&mut kind as *mut i32).cast(),
            &mut kind_len,
        ) == SOCKET_ERROR
            || kind != SOCK_STREAM
        {
            return bind(socket, address, length);
        }
        ipv4.sin_addr.S_un.S_addr = u32::from_ne_bytes([127, 0, 0, 1]);
        let result = bind(
            socket,
            (&ipv4 as *const SOCKADDR_IN).cast(),
            size_of::<SOCKADDR_IN>() as i32,
        );
        if result == 0 {
            let mut bound: SOCKADDR_IN = std::mem::zeroed();
            let mut size = size_of::<SOCKADDR_IN>() as i32;
            if getsockname(socket, (&mut bound as *mut SOCKADDR_IN).cast(), &mut size) == 0
                && bound.sin_addr.S_un.S_addr == u32::from_ne_bytes([127, 0, 0, 1])
            {
                let port = u16::from_be(bound.sin_port);
                if let Ok(mut ports) = TRACKED_PORTS.lock() {
                    ports.insert(port);
                    let _ = VERIFIED_PORT.try_with(|p| p.set(port));
                }
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Networking::WinSock::{
        IPPROTO_TCP, WSADATA, WSAStartup, closesocket, socket,
    };
    #[test]
    fn tunnel_bind_is_restricted_before_listen() {
        unsafe {
            let mut data: WSADATA = std::mem::zeroed();
            assert_eq!(WSAStartup(0x202, &mut data), 0);
            let socket = socket(AF_INET.into(), SOCK_STREAM, IPPROTO_TCP);
            let mut address: SOCKADDR_IN = std::mem::zeroed();
            address.sin_family = AF_INET;
            let scope = ForwardScope::new();
            assert_eq!(
                local_bind(
                    socket,
                    (&address as *const SOCKADDR_IN).cast(),
                    size_of::<SOCKADDR_IN>() as i32
                ),
                0
            );
            let mut size = size_of::<SOCKADDR_IN>() as i32;
            assert_eq!(
                getsockname(socket, (&mut address as *mut SOCKADDR_IN).cast(), &mut size),
                0
            );
            assert_eq!(
                address.sin_addr.S_un.S_addr,
                u32::from_ne_bytes([127, 0, 0, 1])
            );
            assert!(scope.verified(u16::from_be(address.sin_port)));
            release(u16::from_be(address.sin_port));
            closesocket(socket);
        }
    }
}
