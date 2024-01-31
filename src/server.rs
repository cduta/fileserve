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

const CR                  : u8      = 13;
const LF                  : u8      = 10;
const HYPHEN              : u8      = 45;
const BUFFER_SIZE         : usize   = 8096;
const STREAM_BLOCK_IN_SECS: u64     = 5;
const CRLF                : [u8; 2] = [CR,LF];
const DASH                : [u8; 1] = [HYPHEN];
const HEADER_END          : [u8; 4] = [CR,LF,CR,LF];
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
  fn compile_header_info(lines: Vec<&str>, skip: usize) -> HashMap<String, String> {
    lines
    .iter()
    .skip(skip)
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
      info   : Request::compile_header_info(lines, 1)
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
  dirs = dirs.into_iter().map(|s| ["<button class=\"btnLink invisible\" >💾</button> <a href=\"".as_bytes().to_vec(),s.clone(),"/\"><button class=\"btnLink\">".as_bytes().to_vec(),s,"/</button></a>".as_bytes().to_vec()].concat()).collect();
  files.sort();
  files = files.into_iter().map(|s| ["<button class=\"btnLink\" onclick=\"javascript:download('".as_bytes().to_vec(),s.clone(),"', true)\" >💾</button> <button class=\"btnLink\" onclick=\"javascript:download('".as_bytes().to_vec(),s.clone(),"', false)\" onmouseenter=\"javascript:show_preview('".as_bytes().to_vec(),s.clone(),"');\" onmousedown=\"javascript:show_preview('".as_bytes().to_vec(),s.clone(),"');\"') onmouseleave=\"javascript:hide_preview();\" onmouseout=\"javascript:hide_preview();\" onmouseup=\"javascript:hide_preview();\">".as_bytes().to_vec(),s,"</button>".as_bytes().to_vec()].concat() ).collect();
  dirs.append(files.as_mut());
  Ok(dirs.into_iter().fold(Vec::new(), |mut acc: Vec<u8>, mut entry| { acc.append(&mut entry); acc.append("<br>".as_bytes().to_vec().as_mut()); acc }))
}

fn read_until_done<F>(stream: &mut TcpStream, mut f: F) -> Result<(), ServerError>
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

  fn compile_content_disposition(header_vec: Vec<u8>) -> Result<HashMap<String, String>, ServerError> {
    let mut content_disposition = HashMap::new();
    let content_header_info = Request::compile_header_info(String::from_utf8_lossy(header_vec.as_slice()).split("\r\n").collect::<Vec<&str>>(), 0);
    if let Some(content_disposition_string) = content_header_info.get("Content-Disposition") {
      content_disposition_string
      .split("; ")
      .filter_map(|s| s.split_once("="))
      .for_each(|(key,value)| { content_disposition.insert(key.to_string(), value.trim_matches('"').to_string()); });

      if !content_disposition.contains_key("filename") {
        return Err(ServerError::HTTPParseError("Content Incomplete; no filename found".to_string()));
      }
    } else {
      return Err(ServerError::HTTPParseError("Content Incomplete; no Content-Disposition found".to_string()));
    }
    Ok(content_disposition)
  }

  fn get_content_disposition(body_vec: &mut Vec<u8>, stream: &mut TcpStream, total_read: &mut usize, content_length: usize) -> Result<HashMap<String, String>, ServerError> {
    let mut header_vec = Vec::new();
    if let Some(cutoff) = body_vec.windows(4).position(|w| w.cmp(&HEADER_END).is_eq()) {
        header_vec = body_vec[..cutoff].to_vec();
        *body_vec = body_vec[cutoff+HEADER_END.len()..].to_vec();
    } else { // Otherwise, read until header is ready
      header_vec.append(body_vec);
      read_until_done(stream, |read: usize, done: &mut bool, cumulative_buffer: &mut Vec<u8>| {
        *total_read += read;
        if let Some(cutoff) = cumulative_buffer.windows(4).position(|w| w.cmp(&HEADER_END).is_eq()) {
          header_vec.append(&mut cumulative_buffer[..cutoff].to_vec());
          *body_vec = cumulative_buffer[cutoff+HEADER_END.len()..].to_vec();
          *done = true;
        }
      })?;
    }
    println!("### BEGIN CONTENT HEADER (Read {total_read}/{content_length}) ###");
    println!("{}{}", String::from_utf8_lossy(&header_vec), String::from_utf8_lossy(&HEADER_END));
    println!("### END CONTENT HEADER ###");
    Ok(compile_content_disposition(header_vec)?)
  }

  fn create_file(path: &String, content_disposition: HashMap<String, String>) -> Result<File, ServerError>{
    match content_disposition.get("filename") {
      Some(file_name) =>
        if file_name.is_empty() {
          Err(ServerError::HTTPParseError("Uploading File Failed: No file name found".to_string()))
        } else {
          Ok(File::create(["files/",path.as_str(),file_name].concat())?)
        },
      None => Err(ServerError::HTTPParseError("Uploading File Failed: No file name found".to_string()))
    }
  }

  let first_separator = &[&DASH, content_separator.as_bytes(), &CRLF].concat();
  let mid_separator = &[&[CR], &[LF], &DASH, &DASH, content_separator.as_bytes()].concat();
  let mut total_read = body_vec.len()+1;

  if content_length <= first_separator.len() {
    return Ok(());
  }

  // If the body is too short, read more
  if body_vec.len() < first_separator.len() {
    read_until_done(stream, |read: usize, done: &mut bool, cumulative_buffer: &mut Vec<u8>| {
      total_read += read;
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
  // println!("{}", String::from_utf8_lossy(&first_separator));
  body_vec = body_vec.split_at(first_separator.len()).1.to_vec();

  let mut part_complete = false;

  let mut content_disposition = get_content_disposition(&mut body_vec, stream, &mut total_read, content_length)?;
  let mut file = create_file(&path, content_disposition)?;

  // While content is not complete
  while total_read < content_length {
    //  We may still find a last_separator in the body_vec
    let mut pos = 0;
    while !part_complete
       && total_read < content_length
       && pos+mid_separator.len()+2 <= body_vec.len() {
      if body_vec[pos] == CR {
        //println!("### BEGIN CONTENT BODY (Read {total_read}/{content_length} Write {}/{}) ###", body_vec[..pos].len(), body_vec.len());
        //println!("{}", String::from_utf8_lossy(&body_vec[..pos]));
        file.write_all(&body_vec[..pos])?;
        body_vec = body_vec[pos..].to_vec();
        if body_vec.starts_with(&mid_separator) {
          body_vec = body_vec[mid_separator.len()+2..].to_vec();
          part_complete = true;
          //println!("### CONTENT READ ###");
          pos = 0;
        } else {
          pos = 1;
        }
        //println!("### END CONTENT BODY ###");
      } else {
        pos += 1;
      }
    }

    if pos > 0 {
      //println!("### BEGIN CONTENT BODY (Read {total_read}/{content_length} Write {}/{}) ###", body_vec[..pos].len(), body_vec.len());
      //println!("{}", String::from_utf8_lossy(&body_vec[..pos]));
      //println!("### END CONTENT BODY ###");
      file.write_all(&body_vec[..pos])?;
      body_vec = body_vec[pos..].to_vec();
    }

    if !part_complete && total_read < content_length  {
      read_until_done(stream, |read: usize, done: &mut bool, cumulative_buffer: &mut Vec<u8>| {
        total_read += read;
        body_vec.append(cumulative_buffer);
        *done = body_vec.len() >= mid_separator.len();
      })?;
    } else if total_read < content_length {
      content_disposition = get_content_disposition(&mut body_vec, stream, &mut total_read, content_length)?;
      file = create_file(&path, content_disposition)?;
      part_complete = false;
    }
  }
  Ok(())
}

fn serve(mut stream: TcpStream) -> Result<(), ServerError> {
  let mut header_vec: Vec<u8> = Vec::new();
  let mut body_vec: Vec<u8> = Vec::new();

  // Get Request
  read_until_done(&mut stream, |read: usize, done: &mut bool, cumulative_buffer: &mut Vec<u8>| {
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
              "create_directory" => {
                if header.url.is_empty() {
                  (ok, "Can't create directory without name...".as_bytes().to_vec())
                } else {
                  let relative_path = ["files",header.url.as_str()].concat();
                  fs::create_dir(relative_path.clone())?;
                  (ok, ["Directory ", relative_path.as_str(), " created..."].concat().as_bytes().to_vec())
                }
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