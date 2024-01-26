use std::{ sync::{ mpsc, Arc, Mutex }, thread, time::Duration };

pub type ThreadPoolError<'a> = &'a str;
type Job = Box<dyn FnOnce() + Send + 'static>;

pub struct ThreadPool {
  workers: Vec<Worker>,
  sender : Option<mpsc::Sender<Job>>,
}

impl ThreadPool {
  pub fn new(size: usize) -> Result<ThreadPool, ThreadPoolError<'static>> {
    if size == 0 { return Err("ThreadPool must have size larger than 0"); }
    let (sender, receiver) = mpsc::channel();
    let receiver = Arc::new(Mutex::new(receiver));
    let mut workers = Vec::with_capacity(size);

    for id in 0..size {
      workers.push(Worker::new(id, Arc::clone(&receiver)));
    }

    Ok(ThreadPool {
      workers,
      sender: Some(sender),
    })
  }

  pub fn execute<F>(&self, f: F)
  where F: FnOnce() + Send + 'static {
    let job = Box::new(f);

    match self.sender.as_ref() {
      Some(s) => s.send(job).unwrap_or_else(|e| println!("Send Worker Error: Sending the job failed. {e}")),
      None    => println!("Send Worker Error: No sender was set")
    }
  }
}

impl Drop for ThreadPool {
  fn drop(&mut self) {
    drop(self.sender.take());

    for worker in &mut self.workers {
      let id = worker.id;
      println!("Shut down worker {id}");

      if let Some(thread) = worker.thread.take() {
        thread.join().expect(format!("Shutdown Worker (id: {id}) Error: Join failed").as_str())
      }
    }
  }
}

struct Worker {
  id    : usize,
  thread: Option<thread::JoinHandle<()>>
}

impl Worker {
  fn new(id: usize, receiver: Arc<Mutex<mpsc::Receiver<Job>>>) -> Worker {
    Worker { id, thread: Some(thread::spawn(move || work(id, receiver))) }
  }
}

fn work(id: usize, receiver: Arc<Mutex<mpsc::Receiver<Job>>>) -> ! {
  loop {
    match receiver.lock() {
      Ok(exclusive_message) =>
        match exclusive_message.recv() {
          Ok(job) => { println!("HTTP request delegated to worker {id}"); job() },
          Err(e)  => { thread::sleep(Duration::from_secs(1)); println!("Running Worker Error: Receiving message failed. {e}") }
        },
      Err(e) => { thread::sleep(Duration::from_secs(1)); println!("Running Worker Error: Receiver lock failed. {e}") }
    }
  }
}