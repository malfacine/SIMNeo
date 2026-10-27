//! Sessão DVRIP/Sofia para dispositivos Jufeng/Xiongmai.

use serde_json::{Value, json};
use sim_core::Channel;
use std::{net::SocketAddr, time::Duration};
use thiserror::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};
use zeroize::Zeroizing;

const DEADLINE: Duration = Duration::from_secs(8);

#[derive(Debug, Error)]
pub enum Error {
    #[error("Falha de comunicação DVRIP: {0}")]
    Io(#[from] std::io::Error),
    #[error("Tempo limite na comunicação DVRIP.")]
    Timeout,
    #[error("Resposta DVRIP inválida ou não suportada.")]
    Protocol,
    #[error("Operação DVRIP recusada (código {0}).")]
    Rejected(i64),
    #[error("Sessão DVRIP encerrada após erro.")]
    Closed,
}

pub struct Session {
    stream: TcpStream,
    session: u32,
    sequence: u32,
    usable: bool,
    pub keep_alive_interval: Duration,
}

impl Session {
    pub async fn connect(
        endpoint: SocketAddr,
        username: &str,
        password: &str,
    ) -> Result<Self, Error> {
        let stream = timeout(DEADLINE, TcpStream::connect(endpoint))
            .await
            .map_err(|_| Error::Timeout)??;
        stream.set_nodelay(true)?;
        let mut session = Self {
            stream,
            session: 0,
            sequence: 0,
            usable: true,
            keep_alive_interval: Duration::from_secs(20),
        };
        let response = session
            .exchange(
                1000,
                json!({"UserName":username,"PassWord":sofia_hash(password),
            "EncryptType":"MD5","LoginType":"DVRIP-Web"}),
            )
            .await?;
        check(&response)?;
        let id = response["SessionID"]
            .as_str()
            .ok_or(Error::Protocol)?
            .trim_start_matches("0x");
        session.session = u32::from_str_radix(id, 16).map_err(|_| Error::Protocol)?;
        if let Some(seconds) = response["AliveInterval"].as_u64() {
            session.keep_alive_interval = Duration::from_secs(seconds.clamp(5, 60));
        }
        Ok(session)
    }

    pub async fn channels(&mut self) -> Result<Vec<Channel>, Error> {
        let response = self
            .exchange(
                1042,
                json!({"Name":"ChannelTitle","SessionID":self.session_string()}),
            )
            .await?;
        check(&response)?;
        let names = response["ChannelTitle"].as_array().ok_or(Error::Protocol)?;
        if names.len() > 1024 {
            return Err(Error::Protocol);
        }
        names
            .iter()
            .enumerate()
            .map(|(number, name)| {
                Ok(Channel {
                    number: number.try_into().map_err(|_| Error::Protocol)?,
                    name: name.as_str().ok_or(Error::Protocol)?.into(),
                })
            })
            .collect()
    }
    pub async fn keep_alive(&mut self) -> Result<(), Error> {
        let response = self
            .exchange(
                1006,
                json!({"Name":"KeepAlive","SessionID":self.session_string()}),
            )
            .await?;
        check(&response)
    }
    pub async fn logout(mut self) -> Result<(), Error> {
        let response = self
            .exchange(1002, json!({"SessionID":self.session_string()}))
            .await?;
        check(&response)?;
        self.stream.shutdown().await?;
        Ok(())
    }
    fn session_string(&self) -> String {
        format!("0x{:08X}", self.session)
    }

    async fn exchange(&mut self, command: u16, payload: Value) -> Result<Value, Error> {
        if !self.usable {
            return Err(Error::Closed);
        }
        self.usable = false;
        let result = timeout(DEADLINE, self.exchange_inner(command, payload))
            .await
            .map_err(|_| Error::Timeout)?;
        if result.is_ok() {
            self.usable = true;
        }
        result
    }
    async fn exchange_inner(&mut self, command: u16, payload: Value) -> Result<Value, Error> {
        let sequence = self.sequence;
        self.sequence = self.sequence.wrapping_add(1);
        let mut bytes = Zeroizing::new(serde_json::to_vec(&payload).map_err(|_| Error::Protocol)?);
        bytes.extend_from_slice(b"\n\0");
        if bytes.len() > 1024 * 1024 {
            return Err(Error::Protocol);
        }
        let mut h = [0; 20];
        h[0] = 0xff;
        h[4..8].copy_from_slice(&self.session.to_le_bytes());
        h[8..12].copy_from_slice(&sequence.to_le_bytes());
        h[14..16].copy_from_slice(&command.to_le_bytes());
        h[16..20].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
        self.stream.write_all(&h).await?;
        self.stream.write_all(&bytes).await?;
        self.stream.read_exact(&mut h).await?;
        let len = u32::from_le_bytes(h[16..20].try_into().unwrap()) as usize;
        let reply = u16::from_le_bytes(h[14..16].try_into().unwrap());
        let reply_sequence = u32::from_le_bytes(h[8..12].try_into().unwrap());
        if h[0] != 0xff
            || h[1] > 1
            || h[12] > 1
            || h[13] != 0
            || reply != command + 1
            || reply_sequence != sequence
            || !(2..=1024 * 1024).contains(&len)
        {
            return Err(Error::Protocol);
        }
        let returned_session = u32::from_le_bytes(h[4..8].try_into().unwrap());
        if self.session != 0 && returned_session != self.session {
            return Err(Error::Protocol);
        }
        let mut bytes = vec![0; len];
        self.stream.read_exact(&mut bytes).await?;
        if bytes[len - 2..] != *b"\n\0" {
            return Err(Error::Protocol);
        }
        serde_json::from_slice(&bytes[..len - 2]).map_err(|_| Error::Protocol)
    }
}

fn check(response: &Value) -> Result<(), Error> {
    let code = response["Ret"].as_i64().ok_or(Error::Protocol)?;
    if code == 100 {
        Ok(())
    } else {
        Err(Error::Rejected(code))
    }
}

pub fn sofia_hash(password: &str) -> String {
    let digest = md5::compute(password.as_bytes());
    const ALPHABET: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    digest
        .0
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| char::from(ALPHABET[(usize::from(pair[0]) + usize::from(pair[1])) % 62]))
        .collect()
}
