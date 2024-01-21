use fileserve::ThreadPool;
use std::fs;
use std::io;
use std::io::prelude::*;
use std::net::TcpListener;
use std::net::TcpStream;
use std::thread;
use std::time::Duration;

fn main() {
  match TcpListener::bind("127.0.0.1:8000") {
    Ok(listener) => {
      match ThreadPool::new(4) {
        Ok(pool) => listen(listener, pool),
        Err(e)   => println!("Create Thread Pool Error: {e}")
      }
    },
    Err(e) => println!("Could not start server: {e}")
  }
  println!("Shutting down...OK");
}

fn listen(listener: TcpListener, pool: ThreadPool) {
  for stream in listener.incoming() {
    match stream {
      Ok(stream)  => pool.execute(move || if let Err(e) = handle_connection(stream) { println!("Request failed: {e}") }),
      Err(e)      => println!("{e}")
    }
  }
}

fn handle_connection(mut stream: TcpStream) -> Result<(), io::Error> {
  let mut buffer = [0; 8096];

  println!("### BEGIN REQUEST ###");
  stream.read(&mut buffer)?;
  println!("{}", String::from_utf8(buffer.to_vec()).unwrap());
  println!("### END REQUEST ###");

  let get = b"GET / HTTP/1.1\r\n";
  let sleep = b"GET /sleep HTTP/1.1\r\n";

  let (status_line, filename) = if buffer.starts_with(get) {
    ("HTTP/1.1 200 OK", "hello.html")
  } else if buffer.starts_with(sleep) {
    thread::sleep(Duration::from_secs(5));
    ("HTTP/1.1 200 OK", "hello.html")
  } else {
    ("HTTP/1.1 404 NOT FOUND", "404.html")
  };

  let contents = fs::read_to_string(filename)?;

  let response = format!(
    "{}\r\nContent-Length: {}\r\n\r\n{}",
    status_line,
    contents.len(),
    contents
  );

  stream.write_all(response.as_bytes())?;
  stream.flush()?;
  Ok(())
}
