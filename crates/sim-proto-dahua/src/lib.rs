//! Sessão Dahua com autenticação normal e consultas de leitura.

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
const MAX_PAYLOAD: usize = 1024 * 1024;

#[derive(Clone, Copy)]
pub enum WireMode {
    Dhip,
    Legacy,
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("Falha de comunicação com o DVR: {0}")]
    Io(#[from] std::io::Error),
    #[error("Tempo limite na comunicação com o DVR.")]
    Timeout,
    #[error("Resposta de protocolo inválida ou não suportada.")]
    Protocol,
    #[error("O DVR retornou uma resposta de gravações incompatível: {0}.")]
    RecordingProtocol(&'static str),
    #[error("Autenticação recusada pelo DVR (código {0}).")]
    Authentication(i64),
    #[error("Método recusado pelo DVR (código {0}).")]
    Rejected(i64),
    #[error("Sessão encerrada após erro de comunicação.")]
    Closed,
}

pub struct Session {
    stream: TcpStream,
    mode: WireMode,
    session: u32,
    id: u32,
    usable: bool,
    pub keep_alive_interval: Duration,
}

impl Session {
    pub async fn connect(
        endpoint: SocketAddr,
        username: &str,
        password: &str,
        mode: WireMode,
    ) -> Result<Self, Error> {
        timeout(DEADLINE, async {
            let stream = TcpStream::connect(endpoint).await?;
            stream.set_nodelay(true)?;
            let mut session = Self {
                stream,
                mode,
                session: 0,
                id: 1,
                usable: true,
                keep_alive_interval: Duration::from_secs(20),
            };
            match mode {
                WireMode::Dhip => session.login_dhip(username, password).await?,
                WireMode::Legacy => session.login_legacy(username, password).await?,
            }
            Ok(session)
        })
        .await
        .map_err(|_| Error::Timeout)?
    }

    async fn login_dhip(&mut self, username: &str, password: &str) -> Result<(), Error> {
        let challenge = self.exchange("global.login", json!({"userName":username,"password":"","clientType":"Web3.0","loginType":"Direct"})).await?;
        let params = &challenge["params"];
        let realm = params["realm"]
            .as_str()
            .ok_or(Error::Authentication(code(&challenge)))?;
        let random = params["random"].as_str().ok_or(Error::Protocol)?;
        if params["encryption"]
            .as_str()
            .is_some_and(|e| e != "Default")
        {
            return Err(Error::Protocol);
        }
        self.session = challenge["session"]
            .as_u64()
            .and_then(|n| n.try_into().ok())
            .ok_or(Error::Protocol)?;
        let result = self
            .exchange(
                "global.login",
                json!({"userName":username,
            "password":digest_password(username,password,realm,random),"clientType":"Web3.0",
            "loginType":"Direct","authorityType":"Default","passwordType":"Default"}),
            )
            .await?;
        if result["result"] != true {
            return Err(Error::Authentication(code(&result)));
        }
        if let Some(id) = result["session"].as_u64() {
            self.session = id.try_into().map_err(|_| Error::Protocol)?;
        }
        if let Some(seconds) = result["params"]["keepAliveInterval"].as_u64() {
            self.keep_alive_interval = Duration::from_secs(seconds.clamp(5, 60));
        }
        Ok(())
    }

    async fn login_legacy(&mut self, username: &str, password: &str) -> Result<(), Error> {
        let mut challenge = [0u8; 32];
        challenge[0] = 0xa0;
        challenge[1] = 1;
        challenge[24..32].copy_from_slice(&[5, 2, 1, 1, 0, 0, 0xa1, 0xaa]);
        self.stream.write_all(&challenge).await?;
        let (head, body) = self.read_frame().await?;
        if head[0] != 0xb0 {
            return Err(Error::Protocol);
        }
        let text = std::str::from_utf8(&body).map_err(|_| Error::Protocol)?;
        let realm = text
            .lines()
            .find_map(|l| l.strip_prefix("Realm:"))
            .map(str::trim)
            .ok_or(Error::Protocol)?;
        let random = text
            .lines()
            .find_map(|l| l.strip_prefix("Random:"))
            .map(str::trim)
            .ok_or(Error::Protocol)?;
        let gen1 = Zeroizing::new(legacy_hash(password));
        let old_input = Zeroizing::new(format!("{username}:{random}:{}", *gen1));
        let old = Zeroizing::new(format!("{:X}", md5::compute(old_input.as_bytes())));
        let auth = Zeroizing::new(format!(
            "{username}&&{}{}",
            digest_password(username, password, realm, random),
            *old
        ));
        let mut header = [0u8; 32];
        header[0] = 0xa0;
        header[1] = 5;
        header[4..8].copy_from_slice(&(auth.len() as u32).to_le_bytes());
        header[24..32].copy_from_slice(&[5, 2, 0, 8, 0, 0, 0xa1, 0xaa]);
        self.stream.write_all(&header).await?;
        self.stream.write_all(auth.as_bytes()).await?;
        let (head, _) = self.read_frame().await?;
        if head[8..10] != [0, 8] {
            return Err(Error::Authentication(i64::from(u16::from_be_bytes([
                head[8], head[9],
            ]))));
        }
        self.session = word(&head, 16);
        Ok(())
    }

    pub async fn channels(&mut self) -> Result<Vec<Channel>, Error> {
        let response = self
            .exchange("configManager.getConfig", json!({"name":"ChannelTitle"}))
            .await?;
        check(&response)?;
        let table = response["params"]["table"]
            .as_array()
            .ok_or(Error::Protocol)?;
        if table.len() > 1024 {
            return Err(Error::Protocol);
        }
        table
            .iter()
            .enumerate()
            .map(|(index, item)| {
                Ok(Channel {
                    number: index.try_into().map_err(|_| Error::Protocol)?,
                    name: item["Name"].as_str().ok_or(Error::Protocol)?.to_owned(),
                })
            })
            .collect()
    }

    pub async fn keep_alive(&mut self) -> Result<(), Error> {
        let response = self
            .exchange(
                "global.keepAlive",
                json!({"timeout":self.keep_alive_interval.as_secs(),"active":true}),
            )
            .await?;
        check(&response)
    }
    pub async fn recordings(
        &mut self,
        channel: u16,
        start: &sim_core::recording::Stamp,
        end: &sim_core::recording::Stamp,
        subtype: u8,
    ) -> Result<Vec<sim_core::recording::Recording>, Error> {
        use sim_core::recording::{Recording, Stamp};
        let created = self
            .exchange("mediaFileFind.factory.create", json!({}))
            .await?;
        if created["result"] == false {
            check(&created)?;
        }
        let object = created["result"]
            .as_u64()
            .ok_or(Error::RecordingProtocol("identificador da busca"))?;
        let result = async {
            let response = self
                .exchange_object(
                    object,
                    "mediaFileFind.findFile",
                    json!({"condition":{
                        "Channel":channel,"StartTime":start.0,"EndTime":end.0,
                        "Types":["dav"],"Flags":["*"],"VideoStream":if subtype==0 {"Main"}else{"Extra1"}
                    }}),
                )
                .await?;
            // This is the RPC equivalent of NET_NO_RECORD_FOUND for findFile.
            // It is deliberately not a global error-code translation.
            if response["result"] == false && code(&response) == 0x11030004 {
                return Ok(Vec::new());
            }
            check(&response)?;
            let mut records = Vec::new();
            let mut pages = 0;
            loop {
                pages += 1;
                if pages > 100 {
                    return Err(Error::Protocol);
                }
                let response = self
                    .exchange_object(object, "mediaFileFind.findNextFile", json!({"count":100}))
                    .await?;
                check(&response)?;
                let params = &response["params"];
                let found = params["found"]
                    .as_u64()
                    .ok_or(Error::RecordingProtocol("quantidade de intervalos"))?;
                // No interval list is needed when the page explicitly reports zero results.
                if found == 0 && params["infos"].is_null() {
                    break;
                }
                let items = params["infos"]
                    .as_array()
                    .ok_or(Error::RecordingProtocol("lista de intervalos"))?;
                if found > 100 || items.len() != found as usize {
                    return Err(Error::Protocol);
                }
                for item in items {
                    let a = Stamp::parse(
                        item["StartTime"]
                            .as_str()
                            .ok_or(Error::RecordingProtocol("data inicial ausente"))?,
                    )
                    .map_err(|_| Error::RecordingProtocol("formato da data inicial"))?;
                    let b = Stamp::parse(
                        item["EndTime"]
                            .as_str()
                            .ok_or(Error::RecordingProtocol("data final ausente"))?,
                    )
                    .map_err(|_| Error::RecordingProtocol("formato da data final"))?;
                    if b > a && b > *start && a < *end {
                        records.push(Recording {
                            channel,
                            start: a,
                            end: b,
                            kind: item["Flags"]
                                .as_array()
                                .map(|flags| {
                                    flags
                                        .iter()
                                        .filter_map(Value::as_str)
                                        .collect::<Vec<_>>()
                                        .join(",")
                                })
                                .unwrap_or_else(|| "Gravação".into()),
                        });
                    }
                }
                if records.len() > 10000 {
                    return Err(Error::Protocol);
                }
                if found == 0 {
                    break;
                }
            }
            Ok(records)
        }
        .await;
        let closed = self
            .exchange_object(object, "mediaFileFind.close", json!({}))
            .await;
        let destroyed = self
            .exchange_object(object, "mediaFileFind.destroy", json!({}))
            .await;
        if result.is_ok() {
            check(&closed?)?;
            check(&destroyed?)?;
        }
        result
    }

    async fn exchange_object(
        &mut self,
        object: u64,
        method: &str,
        params: Value,
    ) -> Result<Value, Error> {
        self.exchange_with_object(method, params, Some(object))
            .await
    }
    pub async fn logout(mut self) -> Result<(), Error> {
        // O login binário legado não cria uma sessão global.login JSON.
        // Nesse modo o encerramento é por fechamento TCP; não alegamos ACK
        // de global.logout, que o DVR de teste rejeita com login challenge.
        if matches!(self.mode, WireMode::Dhip) {
            let response = self.exchange("global.logout", Value::Null).await?;
            check(&response)?;
        }
        self.stream.shutdown().await?;
        Ok(())
    }

    async fn exchange(&mut self, method: &str, params: Value) -> Result<Value, Error> {
        self.exchange_with_object(method, params, None).await
    }
    async fn exchange_with_object(
        &mut self,
        method: &str,
        params: Value,
        object: Option<u64>,
    ) -> Result<Value, Error> {
        if !self.usable {
            return Err(Error::Closed);
        }
        self.usable = false;
        let result = timeout(DEADLINE, self.exchange_inner(method, params, object))
            .await
            .map_err(|_| Error::Timeout)?;
        if result.is_ok() {
            self.usable = true;
        }
        result
    }

    async fn exchange_inner(
        &mut self,
        method: &str,
        params: Value,
        object: Option<u64>,
    ) -> Result<Value, Error> {
        let id = self.id;
        self.id = self.id.wrapping_add(1);
        let mut message = json!({"id":id,"session":self.session,"method":method,"params":params});
        if let Some(object) = object {
            message["object"] = json!(object);
        }
        let bytes = Zeroizing::new(serde_json::to_vec(&message).map_err(|_| Error::Protocol)?);
        if bytes.len() > MAX_PAYLOAD {
            return Err(Error::Protocol);
        }
        let mut head = [0; 32];
        let length = (bytes.len() as u32).to_le_bytes();
        match self.mode {
            WireMode::Dhip => {
                head[0] = 0x20;
                head[4..8].copy_from_slice(b"DHIP");
                head[8..12].copy_from_slice(&self.session.to_le_bytes());
                head[12..16].copy_from_slice(&id.to_le_bytes());
                head[16..20].copy_from_slice(&length);
                head[24..28].copy_from_slice(&length);
            }
            WireMode::Legacy => {
                head[0] = 0xf6;
                head[4..8].copy_from_slice(&length);
                head[8..12].copy_from_slice(&id.to_le_bytes());
                head[16..20].copy_from_slice(&length);
                head[24..28].copy_from_slice(&self.session.to_le_bytes());
            }
        }
        self.stream.write_all(&head).await?;
        self.stream.write_all(&bytes).await?;
        for _ in 0..32 {
            let (_, bytes) = self.read_frame().await?;
            let end = bytes
                .iter()
                .rposition(|b| !matches!(b, 0 | b'\n' | b'\r'))
                .map_or(0, |p| p + 1);
            let response: Value =
                serde_json::from_slice(&bytes[..end]).map_err(|_| Error::Protocol)?;
            if response["id"].as_u64() == Some(u64::from(id)) {
                return Ok(response);
            }
            if response.get("method").is_none() {
                return Err(Error::Protocol);
            }
        }
        Err(Error::Protocol)
    }

    async fn read_frame(&mut self) -> Result<([u8; 32], Vec<u8>), Error> {
        let mut header = [0; 32];
        self.stream.read_exact(&mut header).await?;
        let len = match self.mode {
            WireMode::Dhip if header[..8] == [0x20, 0, 0, 0, b'D', b'H', b'I', b'P'] => {
                let len = word(&header, 16);
                if word(&header, 24) != len || word(&header, 28) != 0 {
                    return Err(Error::Protocol);
                }
                len
            }
            WireMode::Legacy if header[0] == 0xb0 => word(&header, 4),
            WireMode::Legacy if header[0] == 0xf6 => {
                let len = word(&header, 4);
                if word(&header, 16) != len {
                    return Err(Error::Protocol);
                }
                len
            }
            _ => return Err(Error::Protocol),
        } as usize;
        if len > MAX_PAYLOAD {
            return Err(Error::Protocol);
        }
        let mut body = vec![0; len];
        self.stream.read_exact(&mut body).await?;
        Ok((header, body))
    }
}

fn word(header: &[u8; 32], offset: usize) -> u32 {
    u32::from_le_bytes(header[offset..offset + 4].try_into().unwrap())
}
fn code(response: &Value) -> i64 {
    response["error"]["code"].as_i64().unwrap_or(-1)
}
fn check(response: &Value) -> Result<(), Error> {
    if response["result"] == true {
        Ok(())
    } else {
        Err(Error::Rejected(code(response)))
    }
}

pub fn digest_password(username: &str, password: &str, realm: &str, random: &str) -> String {
    let input = Zeroizing::new(format!("{username}:{realm}:{password}"));
    let first = Zeroizing::new(format!("{:X}", md5::compute(input.as_bytes())));
    let second = Zeroizing::new(format!("{username}:{random}:{}", *first));
    format!("{:X}", md5::compute(second.as_bytes()))
}

fn legacy_hash(password: &str) -> String {
    let digest = md5::compute(password.as_bytes());
    digest
        .0
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let n = ((u16::from(pair[0]) + u16::from(pair[1])) % 62) as u8;
            char::from(match n {
                0..=9 => n + b'0',
                10..=35 => n + 55,
                _ => n + 61,
            })
        })
        .collect()
}
