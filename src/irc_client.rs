use std::io::{self, BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::mpsc::Sender;
use std::thread::{self, JoinHandle};
use std::time::Duration;

// Unified error type
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
        // Clean up existing connection if any
        if self.stream.is_some() {
            self.disconnect()
                .map_err(|e| format!("Error disconnecting: {}", e))?;
        }

        let address = format!("{}:{}", server, port);
        match TcpStream::connect(address) {
            Ok(stream) => {
                // Set read timeout to avoid hanging indefinitely
                if let Err(e) = stream.set_read_timeout(Some(Duration::from_secs(30))) {
                    return Err(format!("Failed to set read timeout: {}", e));
                }

                // Set write timeout
                if let Err(e) = stream.set_write_timeout(Some(Duration::from_secs(10))) {
                    return Err(format!("Failed to set write timeout: {}", e));
                }

                self.stream = Some(stream);
                self.server = server.to_string();
                Ok(())
            }
            Err(e) => Err(format!("Failed to connect: {}", e)),
        }
    }

    pub fn disconnect(&mut self) -> Result<()> {
        if self.stream.is_some() {
            // Try to send QUIT message
            let _ = self.quit();
            self.stream = None;
            self.current_channel.clear();
        }
        Ok(())
    }

    pub fn register(&mut self) -> Result<()> {
        if let Some(stream) = &mut self.stream {
            // Send NICK command
            self.send_raw(&format!("NICK {}\r\n", self.nickname))?;

            // Send USER command (username, hostname, servername, real name)
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
            match stream.write_all(message.as_bytes()) {
                Ok(_) => {
                    // Ensure message is sent immediately
                    match stream.flush() {
                        Ok(_) => Ok(()),
                        Err(e) => Err(format!("Failed to flush message: {}", e)),
                    }
                }
                Err(e) => Err(format!("Failed to send message: {}", e)),
            }
        } else {
            Err("Not connected to server".to_string())
        }
    }

    // Start a background thread to receive messages, returning the thread handle
    pub fn start_receiver(&mut self, tx: Sender<String>) -> Result<JoinHandle<()>> {
        if let Some(stream) = &self.stream {
            let stream_clone = match stream.try_clone() {
                Ok(clone) => clone,
                Err(e) => return Err(format!("Failed to clone stream: {}", e)),
            };

            // Clone nickname for use in the thread
            let nickname = self.nickname.clone();

            let handle = thread::spawn(move || {
                Self::receiver_loop(stream_clone, tx, nickname);
            });

            Ok(handle)
        } else {
            Err("Not connected to server".to_string())
        }
    }

    // Separate function for the receiver loop - makes the code more maintainable
    fn receiver_loop(stream: TcpStream, tx: Sender<String>, nickname: String) {
        // Create a separate stream for sending PONG responses
        let mut pong_stream = match stream.try_clone() {
            Ok(clone) => clone,
            Err(e) => {
                let _ = tx.send(format!("Error: Failed to clone stream for PONG: {}", e));
                return;
            }
        };

        // Use the original stream for reading
        let reader = BufReader::new(stream);

        for line in reader.lines() {
            match line {
                Ok(msg) => {
                    // Process the message with the separate pong_stream
                    if let Some(processed) =
                        Self::process_message(&msg, &mut pong_stream, &nickname)
                    {
                        // Only send the message if processing returned something
                        if let Err(e) = tx.send(processed) {
                            eprintln!("Failed to send message to channel: {}", e);
                            break;
                        }
                    }
                }
                Err(e) => {
                    // Only send actual errors, not just socket closing
                    if e.kind() != io::ErrorKind::ConnectionAborted
                        && e.kind() != io::ErrorKind::ConnectionReset
                    {
                        let _ = tx.send(format!("Error reading from server: {}", e));
                    }
                    break;
                }
            }
        }

        // Send notification that connection was closed
        let _ = tx.send("Connection to server closed.".to_string());
    }

    // Process a single IRC message
    fn process_message(msg: &str, stream: &mut TcpStream, nickname: &str) -> Option<String> {
        // Handle PING messages immediately
        if msg.starts_with("PING") {
            let pong = msg.replace("PING", "PONG");
            // Send PONG response
            if let Err(e) = stream.write_all(format!("{}\r\n", pong).as_bytes()) {
                return Some(format!("Failed to send PONG: {}", e));
            }
            if let Err(e) = stream.flush() {
                return Some(format!("Failed to flush PONG: {}", e));
            }
            return Some(format!(">>> Server ping: {}", msg));
        }

        // Check for NickServ messages
        if msg.contains("NickServ") || msg.contains("nickserv") {
            // Parse the message for more precise handling
            let parts: Vec<&str> = msg.splitn(4, ' ').collect();
            if parts.len() >= 4 {
                let sender = parts[0].trim_start_matches(':');
                let command = parts[1];
                let target = parts[2];

                // If it's directed to our nickname and is from NickServ
                if (command == "NOTICE" || command == "PRIVMSG")
                    && target == nickname
                    && (sender.contains("NickServ") || sender.ends_with("!NickServ@services"))
                {
                    return Some(format!("!!! NICKSERV: {}", msg));
                }
            }
        }

        // Standard message processing
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
        // Ensure we attempt to quit and clean up when the client is dropped
        let _ = self.quit();
    }
}

