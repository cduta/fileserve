use std::net::TcpListener;
use std::fs;
use std::io::prelude::*;
use std::net::TcpStream;
use std::thread;
use std::time::Duration;

mod threadpool;
mod error;

use threadpool::ThreadPool;
use error::ServerError;

fn serve(mut stream: TcpStream) -> Result<(), ServerError> {
  let mut buffer = [0; 8096];

  // Get Request
  println!("### BEGIN REQUEST ###");
  stream.read(&mut buffer)?;
  let req_string = String::from_utf8(buffer.to_vec())?;
  println!("{req_string}");
  println!("### END REQUEST ###");

  // Parse Request
  let get = b"GET / HTTP/1.1\r\n";
  let sleep = b"GET /sleep HTTP/1.1\r\n";

  // Evaluate Request
  let (status_line, filename) = if buffer.starts_with(get) {
    ("HTTP/1.1 200 OK", "hello.html")
  } else if buffer.starts_with(sleep) {
    thread::sleep(Duration::from_secs(5));
    ("HTTP/1.1 200 OK", "hello.html")
  } else {
    ("HTTP/1.1 404 NOT FOUND", "404.html")
  };

  // Build content for response
  let contents = fs::read_to_string(filename)?;

  // Build response
  let response = format!(
    "{}\r\nContent-Length: {}\r\n\r\n{}",
    status_line,
    contents.len(),
    contents
  );

  // Respond
  stream.write_all(response.as_bytes())?;
  stream.flush()?;

  // Done
  Ok(())
}

fn listen(listener: TcpListener, pool: ThreadPool) {
  for stream in listener.incoming() {
    match stream {
      Ok(stream)  => pool.execute(move || if let Err(e) = serve(stream) { println!("Request failed: {e}") }),
      Err(e)      => println!("{e}")
    }
  }
}

pub fn run() -> i32 {
  let mut rc = 0;

  match TcpListener::bind("127.0.0.1:8000") {
    Ok(listener) => {
      match ThreadPool::new(16) {
        Ok(pool) => listen(listener, pool),
        Err(e)   => { println!("Create Thread Pool Error: {e}"); rc = 1; }
      }
    },
    Err(e) => { println!("Could not start server: {e}"); rc = 2; }
  }
  println!("Shutting down...OK");
  rc
}