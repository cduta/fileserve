use std::collections::HashMap;
use std::fmt::Display;
use std::fs::File;
use std::net::TcpListener;
use std::time::Duration;
use std::{fs, io, thread};
use std::io::{prelude::*, ErrorKind};
use std::net::TcpStream;
use std::os::unix::ffi::OsStrExt;

mod threadpool;
mod error;

use threadpool::ThreadPool;
use error::ServerError;

const BUFFER_SIZE         : usize   = 8096;
const STREAM_BLOCK_IN_SECS: u64     = 5;
const CRLF                : [u8; 2] = [13,10];
const DASH_DASH           : [u8; 2] = [45,45];
const HEADER_END          : [u8; 4] = [13,10,13,10];
const FIRST_SEPARATOR     : [u8; 2] = [45,45];
const BODY_SEPARATOR      : [u8; 4] = [13,10,45,45];
const MAX_RETRIES         : u8      = 5;

enum HTTPRequestType { GET, POST }

impl Display for HTTPRequestType {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      HTTPRequestType::GET  => write!(f, "GET"),
      HTTPRequestType::POST => write!(f, "POST")
    }
  }
}

impl TryFrom<&str> for HTTPRequestType {
    type Error = ServerError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
      match value {
        "GET"  => Ok(HTTPRequestType::GET),
        "POST" => Ok(HTTPRequestType::POST),
        _      => Err(ServerError::HTTPParseError(format!("HTTP Parse Error: Cannot convert {value} to HTTPRequestType")))
      }
    }
}

type HTTPSettings = HashMap<String,String>;

struct Request {
  r_type : HTTPRequestType,
  url    : String,
  version: String,
  info   : HTTPSettings
}

impl Request {
  fn parse_header_info(lines: Vec<&str>) -> HashMap<String, String> {
    lines
    .iter()
    .skip(1)
    .fold(HashMap::new(), |mut acc, s| {
      if let Some((name, value)) = s.split_once(": ") {
        if let Some(prev) = acc.insert(name.to_string(), value.trim().to_string()) {
          println!("HTTP Parse Warning: Duplicate entry {name} replaces {prev} with {value}")
        }
      } else {
        println!("HTTP Parse Warning: Malformed header ({s})");
      }
      acc
    })
  }

  fn parse_header(req_string: String) -> Result<Self,ServerError> {
    let lines = req_string
      .split("\r\n")
      .collect::<Vec<&str>>();

    if lines.is_empty() { return Err(ServerError::HTTPParseError(format!("HTTP Parse Error: Invalid request string ({req_string})"))) }

    let header = lines[0].split(' ').collect::<Vec<&str>>();

    if header.len() != 3 { return Err(ServerError::HTTPParseError(format!("HTTP Parse Error: Malformed header ({})", lines[0]))) }

    Ok(Self {
      r_type : HTTPRequestType::try_from(header[0])?,
      url    : header[1].to_string(),
      version: header[2].to_string(),
      info   : Request::parse_header_info(lines)
    })
  }
}

impl Display for Request {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "Request Type: {}\nVersion: {}\nURL: {}\nSettings: {:?}", self.r_type, self.version, self.url, self.info)
  }
}

fn compile_response(status_line: &str, contents: Vec<u8>) -> Vec<u8> {
  [status_line.as_bytes(),
   "\r\nContent-Length:".as_bytes(),
   contents.len().to_string().as_bytes(),
   "\r\n\r\n".as_bytes(),
   &contents].concat()
}

fn dir(relative_path: String) -> Result<Vec<u8>, ServerError>  {
  let absolute_path = format!("files/{relative_path}");
  let (mut dirs, mut files): (Vec<Vec<u8>>,Vec<Vec<u8>>) = fs::read_dir(&absolute_path)?.fold(
    (Vec::new(),Vec::new()),
    |mut acc, r_entry| {
      match r_entry {
        Ok(entry) => {
          match entry.metadata() {
            Ok(md) if md.is_dir()  => acc.0.push(entry.file_name().as_bytes().to_vec()),
            Ok(md) if md.is_file() => acc.1.push(entry.file_name().as_bytes().to_vec()),
            Ok(_)                  => println!("List Dir Error: {:?} is neither file nor dir", entry.file_name()),
            Err(e)                 => println!("List Dir Error: {}", e.to_string())
          } acc
        },
        Err(e)    => { println!("List Dir Error: {}", e.to_string()); acc }
      }
    }
  );

  dirs.sort();
  dirs = dirs.into_iter().map(|s| ["<a href=\"".as_bytes().to_vec(),s.clone(),"/\"><button class=\"btnLink\">".as_bytes().to_vec(),s,"/</button></a>".as_bytes().to_vec()].concat()).collect();
  files.sort();
  files = files.into_iter().map(|s| ["<button class=\"btnLink\" onclick=\"javascript:download('".as_bytes().to_vec(),s.clone(),"')\" onmouseenter=\"javascript:show_preview('".as_bytes().to_vec(),s.clone(),"');\" onmousedown=\"javascript:show_preview('".as_bytes().to_vec(),s.clone(),"');\"') onmouseleave=\"javascript:hide_preview();\" onmouseout=\"javascript:hide_preview();\" onmouseup=\"javascript:hide_preview();\">".as_bytes().to_vec(),s,"</button>".as_bytes().to_vec()].concat() ).collect();
  dirs.append(files.as_mut());
  Ok(dirs.into_iter().fold(Vec::new(), |mut acc: Vec<u8>, mut entry| { acc.append(&mut entry); acc.append("<br>".as_bytes().to_vec().as_mut()); acc }))
}

fn read_via<F>(stream: &mut TcpStream, mut f: F) -> Result<(), ServerError>
where F: FnMut(usize, &mut bool, &mut Vec<u8>) {
  let mut buffer = [0; BUFFER_SIZE];
  let mut done = false;
  let mut retries = 0;
  let mut cumulative_buffer: Vec<u8> = Vec::new();
  while !done {
    if retries > MAX_RETRIES { return Err(ServerError::TransportError(io::Error::from_raw_os_error(22))); }
    match stream.read(&mut buffer) {
      Ok(read) => {
        if read == 0 { thread::sleep(Duration::from_secs(1)); retries += 1 } else {
          retries = 0;
          cumulative_buffer.append(&mut buffer[0..read].to_vec());
          f(read, &mut done, &mut cumulative_buffer);
        }
      },
      Err(e)   => {
        retries += 1;
        match e.kind() {
          ErrorKind::WouldBlock => { thread::sleep(Duration::from_secs(STREAM_BLOCK_IN_SECS)); },
          _ => { thread::sleep(Duration::from_secs(1)); println!("Header Read Error: {e}") }
        }
      }
    }
  }
  Ok(())
}

fn upload_files(    stream       : &mut TcpStream,
                mut body_vec     : Vec<u8>,
                path             : String,
                content_separator: String,
                content_length   : usize) -> Result<(), ServerError> {
  let first_separator = &[&DASH_DASH,&FIRST_SEPARATOR,content_separator.as_bytes(), &CRLF].concat();
  let mut header_vec = Vec::new();

  if content_length <= first_separator.len() {
    return Ok(());
  }

  // If the body is too short, read more
  if body_vec.len() < first_separator.len() {
    read_via(stream, |_: usize, done: &mut bool, cumulative_buffer: &mut Vec<u8>| {
      if body_vec.len() + cumulative_buffer.len() >= first_separator.len() {
        body_vec.append(cumulative_buffer);
        *done = true;
      }
    })?;
  }

  // If the body does not start with the first seperator, the body is malformed
  if !body_vec.starts_with(&first_separator) {
    return Err(ServerError::HTTPParseError("Content malformed; first separator not found".to_string()))
  }

  // Remove first separator from body_vec
  body_vec = body_vec.split_at(first_separator.len()).1.to_vec();

  // (*)
  // If we already read part of the body, split accordingly
  if let Some(cutoff) = body_vec.windows(4).position(|w| w.cmp(&HEADER_END).is_eq()) {
    (header_vec, body_vec) = {
      let (header_slice, body_slice) = body_vec.split_at(cutoff+HEADER_END.len());
      (header_slice.to_vec(), body_slice.to_vec())
    };
  } else { // Otherwise, read until header is ready
    read_via(stream, |_: usize, done: &mut bool, cumulative_buffer: &mut Vec<u8>| {
      if let Some(cutoff) = cumulative_buffer.windows(4).position(|w| w.cmp(&HEADER_END).is_eq()) {
        body_vec = {
          let (header_slice, body_slice) = cumulative_buffer.split_at(cutoff+HEADER_END.len());
          header_vec.append(&mut header_slice.to_vec());
          body_slice.to_vec()
        };
        *done = true;
      }
    })?;
  }

  let content_header_info = Request::parse_header_info(String::from_utf8_lossy(header_vec.as_slice()).split("\r\n").collect::<Vec<&str>>());
  let mut content_disposition = HashMap::new();
  if let Some(content_disposition_string) = content_header_info.get("Content-Disposition") {
    content_disposition_string
    .split("; ")
    .filter_map(|s| s.split_once("="))
    .for_each(|(key,value)| { content_disposition.insert(key, value.trim_matches('"')); });

    if !content_disposition.contains_key("filename") {
      return Err(ServerError::HTTPParseError("Content Incomplete; no filename found".to_string()));
    }
  } else {
    return Err(ServerError::HTTPParseError("Content Incomplete; no Content-Disposition found".to_string()));
  }

  let mut file = File::create(["files/",path.as_str(),content_disposition.get("filename").unwrap()].concat())?;

  //file.write_all(&body_vec)?;

  // 1. Scan the body_vec for `-`
  // 2. If there is one, write every character before it with file.write_all
  // 3. See, if the following characters (if not enough, read more) are the separator
  //    - If not, continue reading and writing
  //    - If so, then remove it and then check if Content-Length has been read.
  //      - If so, it is done. Leave loop.
  //      - If not, continue at (*)

  Ok(())
}


fn serve(mut stream: TcpStream) -> Result<(), ServerError> {
  let mut header_vec: Vec<u8> = Vec::new();
  let mut body_vec: Vec<u8> = Vec::new();

  // Get Request
  read_via(&mut stream, |read: usize, done: &mut bool, cumulative_buffer: &mut Vec<u8>| {
    if cumulative_buffer.len() >= HEADER_END.len() {
      let mut cutoff = HEADER_END.len();
      for window in cumulative_buffer.windows(HEADER_END.len()) {
        if window.cmp(&HEADER_END).is_eq() {
          *done = true;
          if cutoff < read { body_vec = cumulative_buffer[cutoff+1..].to_vec(); }
          break;
        } else {
          header_vec.push(window[0]);
          cutoff+=1;
        }
      }
      if !*done { cumulative_buffer.clear() }
    }
  })?;

  let header_string = String::from_utf8_lossy(&header_vec).to_string();
  println!("### BEGIN HEADER ###");
  println!("{header_string}");
  println!("### END HEADER ###");
  println!("### BEGIN (PARTIAL) BODY ###");
  println!("{}", String::from_utf8_lossy(&body_vec));
  println!("### END (PARTIAL) BODY ###");

  // Parse Header (Tokenize: "<GET|...> <URL> <HTTP/\d+.\d+>\n(<field>:<value>\n)+")
  let header = Request::parse_header(header_string)?;

  let ok        = "HTTP/1.1 200 OK";
  let not_found = "HTTP/1.1 404 NOT FOUND";

  // Evaluate header
  let (status_line, contents) =
    if header.url.contains("..") {
      println!("Server Error: Indirection in path forbidden");
      (not_found, "Woops".as_bytes().to_vec())
    } else {
      let path = header.url.chars().skip(1).collect::<String>();
      match header.r_type {
        HTTPRequestType::GET => {
          if header.url.ends_with("/") {
            (ok, fs::read_to_string("files.html")?.replace("{{Entries}}", String::from_utf8(dir(path)?)?.as_str()).as_bytes().to_vec())
          } else if path.starts_with("static/icons") {
            (ok, fs::read(path)?)
          } else {
            (ok, fs::read(format!("files/{path}"))?)
          }
        },
        HTTPRequestType::POST => {
          if let Some(action) = header.info.get("Action") {
            match action.as_str() {
              "create_file" => {
                let relative_path = ["files",header.url.as_str()].concat();
                fs::create_dir(relative_path.clone())?;
                (ok, ["Directory ", relative_path.as_str(), " created..."].concat().as_bytes().to_vec())
              },
              _ => {
                println!("Server Error: Invalid Action `{action}`");
                (not_found, "Woops".as_bytes().to_vec())
              }
            }
          } else if let (Some(content_separator), Some(content_length)) = (
                header.info.get("Content-Type").and_then(|content_type| content_type.split_once("boundary=").map(|(_,sep)| sep.to_string())),
                header.info.get("Content-Length")) {
            match upload_files(&mut stream, body_vec, path, content_separator, content_length.parse::<usize>()?) {
              Ok(()) => { (ok, ":)".as_bytes().to_vec()) },
              Err(e) => { println!("Server Error: File upload failed. {e}"); (not_found, "Woops".as_bytes().to_vec()) }
            }
          } else {
            println!("Server Error: POST is neither an Action nor has a Content-Type");
            (not_found, "Woops".as_bytes().to_vec())
          }
        }
      }
    };

  // Respond
  stream.write_all(compile_response(status_line, contents).as_slice())?;
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

  if let Err(e) = fs::read_dir("files/") {
    if e.kind() == io::ErrorKind::NotFound {
      if let Err(e) = fs::create_dir("files/") {
        println!("Initialize Server Error: {e}"); rc = 1;
      }
    } else if e.kind() != io::ErrorKind::AlreadyExists {
      println!("Initialize Server Error: {e}"); rc = 2;
    }
  }

  if rc == 0 {
    match TcpListener::bind("192.168.178.43:8000") {
      Ok(listener) => {
        match ThreadPool::new(16) {
          Ok(pool) => listen(listener, pool),
          Err(e)   => { println!("Create Thread Pool Error: {e}"); rc = 3; }
        }
      },
      Err(e) => { println!("Could not start server: {e}"); rc = 4; }
    }
    println!("Shutting down...OK");
  }
  rc
}