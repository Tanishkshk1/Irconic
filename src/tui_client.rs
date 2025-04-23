//mod connection/irc_client;

use crate::irc_client::IrcClient;
use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use std::collections::BTreeMap;
use std::io::{self, Write, stdout};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::Duration;

pub fn run_tui_client() -> Result<(), Box<dyn std::error::Error>> {
    // Setup phase - Get user inputs
    println!("OrangeIRC - TUI IRC Client");
    println!("--------------------------");

    // Get user input for connection details
    println!("Enter your nickname:");
    let mut nickname = String::new();
    std::io::stdin().read_line(&mut nickname).unwrap();
    let nickname = nickname.trim();

    println!("Enter the server address (e.g., irc.libera.chat):");
    let mut server = String::new();
    std::io::stdin().read_line(&mut server).unwrap();
    let server = server.trim();

    println!("Enter the port (default: 6667):");
    let mut port_str = String::new();
    std::io::stdin().read_line(&mut port_str).unwrap();
    let port = match port_str.trim().parse::<u16>() {
        Ok(p) if p > 0 => p,
        _ => 6667, // Default port
    };

    // Setup IRC client
    let mut client = IrcClient::new(nickname);

    println!("Connecting to {}:{}...", server, port);
    if let Err(e) = client.connect(server, port) {
        println!("Connection error: {}", e);
        return Ok(());
    }

    println!("Connected! Registering nickname...");
    if let Err(e) = client.register() {
        println!("Registration error: {}", e);
        return Ok(());
    }

    // Create channel for server messages
    let (tx, rx): (Sender<String>, Receiver<String>) = channel();

    if let Err(e) = client.start_receiver(tx.clone()) {
        println!("Failed to start receiver: {}", e);
        return Ok(());
    }

    // Wait for initial server messages
    thread::sleep(Duration::from_secs(1));

    // Initialize TUI
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut input = String::new();
    let mut messages: Vec<String> = vec!["Welcome to OrangeIRC".into()];

    // Add some initial server messages
    while let Ok(msg) = rx.try_recv() {
        messages.push(msg);
    }

    // Commands with descriptions
    let commands: BTreeMap<&str, &str> = BTreeMap::from([
        ("/help", "Display all available commands with descriptions"),
        ("/clear", "Clear the chat window"),
        ("/join", "Join a channel: /join #channel"),
        ("/msg", "Send a private message: /msg target message"),
        ("/nickserv", "Send command to NickServ: /nickserv command"),
        ("/quit", "Exit the application"),
    ]);

    // Tab completion state
    let mut completion_matches: Vec<String> = Vec::new();
    let mut completion_index: usize = 0;
    let mut last_input: String = String::new();

    loop {
        // Check for new messages from server
        while let Ok(msg) = rx.try_recv() {
            messages.push(msg);
            // Keep message list at a reasonable size
            if messages.len() > 1000 {
                messages.remove(0);
            }
        }

        // Draw UI
        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .margin(1)
                .constraints([Constraint::Min(5), Constraint::Length(3)].as_ref())
                .split(f.size());

            // Chat history
            let messages_block = Block::default()
                .title(format!(
                    "Server: {} - Channel: {}",
                    if client.server.is_empty() {
                        "Not connected"
                    } else {
                        &client.server
                    },
                    if client.current_channel.is_empty() {
                        "None"
                    } else {
                        &client.current_channel
                    }
                ))
                .borders(Borders::ALL);

            let message_height = chunks[0].height as usize - 2; // Account for borders
            let messages_to_show = if messages.len() > message_height {
                &messages[messages.len() - message_height..]
            } else {
                &messages[..]
            };

            let msg_paragraph = Paragraph::new(
                messages_to_show
                    .iter()
                    .map(|m| {
                        if m.starts_with("!!!") {
                            Line::from(vec![Span::styled(
                                m,
                                Style::default()
                                    .fg(Color::Yellow)
                                    .add_modifier(Modifier::BOLD),
                            )])
                        } else {
                            Line::from(vec![Span::raw(m)])
                        }
                    })
                    .collect::<Vec<_>>(),
            )
            .block(messages_block)
            .wrap(Wrap { trim: true });

            f.render_widget(msg_paragraph, chunks[0]);

            let input_text = Text::from(input.clone());
            let input_block = Paragraph::new(input_text)
                .block(
                    Block::default()
                        .title(format!(
                            "Input (Current channel: {})",
                            if client.current_channel.is_empty() {
                                "None"
                            } else {
                                &client.current_channel
                            }
                        ))
                        .borders(Borders::ALL),
                )
                .style(Style::default());
            f.render_widget(input_block, chunks[1]);

            // Blinking cursor
            f.set_cursor(chunks[1].x + input.len() as u16 + 1, chunks[1].y + 1);
        })?;

        // Handle input
        if event::poll(std::time::Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Enter => {
                        // Process commands
                        if input.starts_with("/join ") {
                            let channel = &input[6..];
                            if channel.is_empty() {
                                messages.push("Usage: /join #channel".to_string());
                            } else {
                                match client.join_channel(channel) {
                                    Ok(_) => messages.push(format!("Joining channel: {}", channel)),
                                    Err(e) => {
                                        messages.push(format!("Error joining channel: {}", e))
                                    }
                                }
                            }
                        } else if input.starts_with("/msg ") {
                            let parts: Vec<&str> = input[5..].splitn(2, ' ').collect();
                            if parts.len() != 2 {
                                messages.push("Usage: /msg target message".to_string());
                            } else {
                                let target = parts[0];
                                let message = parts[1];

                                match client.send_message(target, message) {
                                    Ok(_) => messages.push(format!("-> *{}* {}", target, message)),
                                    Err(e) => {
                                        messages.push(format!("Error sending message: {}", e))
                                    }
                                }
                            }
                        } else if input.starts_with("/nickserv ") {
                            let command = &input[9..];
                            match client.send_message("NickServ", command) {
                                Ok(_) => messages.push(format!("-> *NickServ* {}", command)),
                                Err(e) => {
                                    messages.push(format!("Error sending to NickServ: {}", e))
                                }
                            }
                        } else if input == "/clear" {
                            messages.clear();
                            messages.push("Chat cleared.".to_string());
                        } else if input == "/quit" || input == "/exit" {
                            let _ = client.quit();
                            break;
                        } else if input == "/help" {
                            messages.push("---- Command Help ----".to_string());
                            for (cmd, desc) in &commands {
                                messages.push(format!("{} - {}", cmd, desc));
                            }
                        } else if !input.is_empty() {
                            // Send message to current channel
                            let current_channel = client.current_channel.clone();
                            if client.current_channel.is_empty() {
                                messages
                                    .push("Join a channel first with /join #channel".to_string());
                            } else {
                                match client.send_message(&current_channel, &input) {
                                    Ok(_) => messages
                                        .push(format!("-> {}: {}", client.current_channel, input)),
                                    Err(e) => {
                                        messages.push(format!("Error sending message: {}", e))
                                    }
                                }
                            }
                        }
                        input.clear();
                    }
                    KeyCode::Char(c) => {
                        input.push(c);
                    }
                    KeyCode::Backspace => {
                        input.pop();
                    }
                    KeyCode::Tab => {
                        if input.starts_with('/') {
                            // Reset match list if input changed
                            if input != last_input {
                                completion_matches = commands
                                    .keys()
                                    .filter(|cmd| cmd.starts_with(&input))
                                    .map(|s| s.to_string())
                                    .collect();
                                completion_index = 0;
                                last_input = input.clone();
                            }

                            if !completion_matches.is_empty() {
                                input = completion_matches[completion_index].clone();
                                completion_index =
                                    (completion_index + 1) % completion_matches.len();
                            }
                        }
                    }
                    KeyCode::Esc => {
                        let _ = client.quit();
                        break;
                    }
                    _ => {}
                }

                // Reset tab-completion if any non-tab key pressed
                if key.code != KeyCode::Tab {
                    completion_matches.clear();
                    completion_index = 0;
                    last_input.clear();
                }
            }
        }
    }

    // Clean up
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    println!("Disconnected. Goodbye!");
    Ok(())
}
