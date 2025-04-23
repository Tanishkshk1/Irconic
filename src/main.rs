mod irc_client;
mod tui_client;

fn main() {
    match tui_client::run_tui_client() {
        Ok(_) => println!("Client exited normally"),
        Err(e) => eprintln!("Error: {}", e),
    }
}



