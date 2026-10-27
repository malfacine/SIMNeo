//! Adaptador local: inclui Scale no PLAY que o demuxer RTSP não expõe.
use std::{io, net::SocketAddr};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};
use zeroize::Zeroizing;

pub struct Proxy {
    pub endpoint: SocketAddr,
    worker: JoinHandle<()>,
}
impl Proxy {
    pub async fn open(upstream: SocketAddr, speed: u32) -> io::Result<Self> {
        if ![2, 4, 8, 16].contains(&speed) {
            return Err(io::Error::other("Velocidade inválida."));
        }
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = listener.local_addr()?;
        let worker = tokio::spawn(async move {
            // Reconexão do demuxer (por exemplo, fallback de GPU) mantém o mesmo Scale.
            while let Ok((client, _)) = listener.accept().await {
                let Ok(server) = TcpStream::connect(upstream).await else {
                    break;
                };
                let (cr, mut cw) = client.into_split();
                let mut cr = BufReader::new(cr);
                let (mut sr, mut sw) = server.into_split();
                tokio::select! {
                    _=async {
                        loop {
                            let mut first=[0]; cr.read_exact(&mut first).await?;
                            if first[0]==b'$' {
                                let mut head=[0;3];cr.read_exact(&mut head).await?;
                                sw.write_all(&first).await?;sw.write_all(&head).await?;
                                let length=u16::from_be_bytes([head[1],head[2]]) as usize;
                                let mut body=vec![0;length];cr.read_exact(&mut body).await?;sw.write_all(&body).await?;
                                continue;
                            }
                            let mut header=Zeroizing::new(vec![first[0]]);
                            while !header.ends_with(b"\r\n\r\n") {
                                if header.len()>=65536{return Err(io::Error::other("Cabeçalho RTSP muito grande."));}
                                let mut byte=[0];cr.read_exact(&mut byte).await?;header.push(byte[0]);
                            }
                            let text=std::str::from_utf8(&header).map_err(io::Error::other)?;
                            let length=text.lines().filter_map(|l|l.split_once(':')).find(|(k,_)|k.eq_ignore_ascii_case("Content-Length"))
                                .map(|(_,v)|v.trim().parse::<usize>()).transpose().map_err(io::Error::other)?.unwrap_or(0);
                            if length>1048576{return Err(io::Error::other("Corpo RTSP muito grande."));}
                            if header.starts_with(b"PLAY ") {
                                let length=header.len()-2;
                                header.truncate(length);
                                header.extend_from_slice(format!("Scale: {speed}.0\r\n\r\n").as_bytes());
                            }
                            sw.write_all(&header).await?;
                            let mut body=Zeroizing::new(vec![0;length]);cr.read_exact(&mut body).await?;sw.write_all(&body).await?;
                        }
                        #[allow(unreachable_code)] Ok::<(),io::Error>(())
                    }=>{},
                    _=tokio::io::copy(&mut sr,&mut cw)=>{},
                }
            }
        });
        Ok(Self { endpoint, worker })
    }
}
impl Drop for Proxy {
    fn drop(&mut self) {
        self.worker.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn adds_scale_only_to_play_without_changing_authentication_or_rtp() {
        for speed in [2, 4, 8, 16] {
            let server = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let proxy = Proxy::open(server.local_addr().unwrap(), speed)
                .await
                .unwrap();
            let verify = async {
                let (mut server, _) = server.accept().await.unwrap();
                let mut header = Vec::new();
                while !header.ends_with(b"\r\n\r\n") {
                    header.push(server.read_u8().await.unwrap());
                }
                let text = String::from_utf8(header).unwrap();
                assert!(text.contains("Authorization: synthetic\r\n"));
                assert!(text.ends_with(&format!("Scale: {speed}.0\r\n\r\n")));
                server
                    .write_all(b"RTSP/1.0 200 OK\r\nCSeq: 1\r\n\r\n$\0\0\x03abc")
                    .await
                    .unwrap();
            };
            let read = async {
                let mut client = TcpStream::connect(proxy.endpoint).await.unwrap();
                client.write_all(b"PLAY rtsp://127.0.0.1/cam/playback RTSP/1.0\r\nCSeq: 1\r\nAuthorization: synthetic\r\n\r\n").await.unwrap();
                let mut response = Vec::new();
                client.read_to_end(&mut response).await.unwrap();
                assert!(response.ends_with(b"$\0\0\x03abc"));
            };
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                tokio::join!(verify, read);
            })
            .await
            .unwrap();
        }
    }
}
