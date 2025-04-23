use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::sync::mpsc::Sender;
use std::thread::{self, JoinHandle};
use std::time::Duration;

type Result<T> = std::result::Result<T, String>;

pub struct IrcClient {
    pub stream: Option<TcpStream>,
    pub nickname: String,
    pub server: String,
    pub current_channel: String,
}

impl IrcClient {
    pub fn new(nickname: &str) -> Self {
        IrcClient {
            stream: None,
            nickname: nickname.to_string(),
            server: String::new(),
            current_channel: String::new(),
        }
    }

    pub fn connect(&mut self, server: &str, port: u16) -> Result<()> {
        if self.stream.is_some() {
            self.disconnect()?;
        }

        let address = format!("{}:{}", server, port);
        match TcpStream::connect(address) {
            Ok(mut stream) => {
                stream
                    .set_read_timeout(Some(Duration::from_secs(30)))
                    .map_err(|e| format!("Failed to set read timeout: {}", e))?;

                stream
                    .set_write_timeout(Some(Duration::from_secs(10)))
                    .map_err(|e| format!("Failed to set write timeout: {}", e))?;

                self.stream = Some(stream);
                self.server = server.to_string();
                Ok(())
            }
            Err(e) => Err(format!("Failed to connect: {}", e)),
        }
    }

    pub fn disconnect(&mut self) -> Result<()> {
        if self.stream.is_some() {
            let _ = self.quit();
            self.stream = None;
            self.current_channel.clear();
        }
        Ok(())
    }

    pub fn register(&mut self) -> Result<()> {
        if let Some(stream) = &mut self.stream {
            self.send_raw(&format!("NICK {}\r\n", self.nickname))?;
            self.send_raw(&format!(
                "USER {} 0 * :{}\r\n",
                self.nickname, self.nickname
            ))?;
            Ok(())
        } else {
            Err("Not connected to server".to_string())
        }
    }

    pub fn join_channel(&mut self, channel: &str) -> Result<()> {
        let result = self.send_raw(&format!("JOIN {}\r\n", channel));
        if result.is_ok() {
            self.current_channel = channel.to_string();
        }
        result
    }

    pub fn send_message(&mut self, target: &str, message: &str) -> Result<()> {
        self.send_raw(&format!("PRIVMSG {} :{}\r\n", target, message))
    }

    pub fn send_raw(&mut self, message: &str) -> Result<()> {
        if let Some(stream) = &mut self.stream {
            stream
                .write_all(message.as_bytes())
                .map_err(|e| format!("Failed to send message: {}", e))?;
            stream
                .flush()
                .map_err(|e| format!("Failed to flush message: {}", e))?;
            Ok(())
        } else {
            Err("Not connected to server".to_string())
        }
    }

    pub fn start_receiver(&mut self, tx: Sender<String>) -> Result<JoinHandle<()>> {
        if let Some(stream) = &self.stream {
            let stream_clone = stream
                .try_clone()
                .map_err(|e| format!("Failed to clone stream: {}", e))?;
            let nickname = self.nickname.clone();

            let handle = thread::spawn(move || {
                Self::receiver_loop(stream_clone, tx, nickname);
            });

            Ok(handle)
        } else {
            Err("Not connected to server".to_string())
        }
    }

    fn receiver_loop(mut stream: TcpStream, tx: Sender<String>, nickname: String) {
        let mut pong_stream = match stream.try_clone() {
            Ok(clone) => clone,
            Err(e) => {
                let _ = tx.send(format!("Error: Failed to clone stream for PONG: {}", e));
                return;
            }
        };

        let mut buffer = [0; 512];
        let mut read_buffer = String::new();

        loop {
            match stream.read(&mut buffer) {
                Ok(0) => break, // Connection closed
                Ok(n) => {
                    read_buffer.push_str(&String::from_utf8_lossy(&buffer[..n]));

                    while let Some(pos) = read_buffer.find("\r\n") {
                        let line = read_buffer[..pos].to_string();
                        read_buffer.drain(..pos + 2);

                        if let Some(processed) =
                            Self::process_message(&line, &mut pong_stream, &nickname)
                        {
                            if tx.send(processed).is_err() {
                                break;
                            }
                        }
                    }
                }
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(100));
                    continue;
                }
                Err(ref e)
                    if e.kind() == io::ErrorKind::ConnectionReset
                        || e.kind() == io::ErrorKind::ConnectionAborted =>
                {
                    break;
                }
                Err(e) => {
                    let _ = tx.send(format!("Error reading from server: {}", e));
                    break;
                }
            }
        }

        let _ = tx.send("Connection to server closed.".to_string());
    }

    fn process_message(msg: &str, stream: &mut TcpStream, nickname: &str) -> Option<String> {
        if msg.starts_with("PING") {
            let pong = msg.replace("PING", "PONG");
            if let Err(e) = stream.write_all(format!("{}\r\n", pong).as_bytes()) {
                return Some(format!("Failed to send PONG: {}", e));
            }
            if let Err(e) = stream.flush() {
                return Some(format!("Failed to flush PONG: {}", e));
            }
            return Some(format!(">>> Server ping: {}", msg));
        }

        if msg.contains("NickServ") || msg.contains("nickserv") {
            let parts: Vec<&str> = msg.splitn(4, ' ').collect();
            if parts.len() >= 4 {
                let sender = parts[0].trim_start_matches(':');
                let command = parts[1];
                let target = parts[2];

                if (command == "NOTICE" || command == "PRIVMSG")
                    && target == nickname
                    && (sender.contains("NickServ") || sender.ends_with("!NickServ@services"))
                {
                    return Some(format!("!!! NICKSERV: {}", msg));
                }
            }
        }

        Some(msg.to_string())
    }

    pub fn quit(&mut self) -> Result<()> {
        if let Some(stream) = &mut self.stream {
            let _ = stream.write_all(b"QUIT :Leaving\r\n");
            let _ = stream.flush();
            Ok(())
        } else {
            Err("Not connected to server".to_string())
        }
    }
}

impl Drop for IrcClient {
    fn drop(&mut self) {
        let _ = self.quit();
    }
}

