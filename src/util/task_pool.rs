// Copyright 2015 The tiny-http Contributors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::sync::{Arc, Mutex, Condvar};
use std::sync::atomic::{Ordering, AtomicUsize};
use std::collections::VecDeque;
use std::time::Duration;
use std::thread;

/// Manages a collection of threads.
///
/// A new thread is created every time all the existing threads are full.
/// Excessive (more than MIN_THREADS) threads die after IDLE_TIME of no work.
// Confusion: Should be ThreadPool or WorkerPool, as we use task to
// refer to something for a Worker Thread to do!!
pub struct TaskPool {
    sharing: Arc<Sharing>,
}

struct Sharing {
    // list of the tasks to be done by worker threads
    todo: Mutex<VecDeque<Box<FnMut() + Send>>>,

    // condvar that will be notified whenever a task is added to `todo`
    condvar: Condvar,

    // number of total worker threads running
    active_tasks: AtomicUsize,  // active_worker_count??

    // number of idle worker threads
    waiting_tasks: AtomicUsize, // waiting_worker_count??
}

/// Minimum number of active threads.
// How can we move this to the .toml file??
static MIN_THREADS: usize = 4;
static IDLE_TIME: Duration = Duration::from_millis(5000);

struct Registration<'a> {
    nb: &'a AtomicUsize
}

impl<'a> Registration<'a> {
    fn new(nb: &'a AtomicUsize) -> Registration<'a> {
        nb.fetch_add(1, Ordering::Release);
        Registration { nb: nb }
    }
}

impl<'a> Drop for Registration<'a> {
    fn drop(&mut self) {
        self.nb.fetch_sub(1, Ordering::Release);
    }
}

impl TaskPool {
    pub fn new() -> TaskPool {
        let pool = TaskPool {
            sharing: Arc::new(Sharing {
                todo: Mutex::new(VecDeque::new()),
                condvar: Condvar::new(),
                active_tasks: AtomicUsize::new(0),
                waiting_tasks: AtomicUsize::new(0),
            }),
        };

        for _ in 0..MIN_THREADS {
            pool.add_thread(None)
        }

        pool
    }

    /// Executes a function in a thread.
    /// If no thread is available, spawns a new one.
    pub fn spawn(&self, code: Box<FnMut() + Send>) {
        let mut queue = self.sharing.todo.lock().unwrap();

        if self.sharing.waiting_tasks.load(Ordering::Acquire) == 0 {
            self.add_thread(Some(code));

        } else {
            queue.push_back(code);
            self.sharing.condvar.notify_one();
        }
    }

    fn add_thread(&self, initial_fn: Option<Box<FnMut() + Send>>) {
        let sharing = self.sharing.clone();

        thread::spawn(move || {
            let sharing = sharing; // redundant? move will do this!
            // increment active_worker_count while we're alive
            let _active_guard = Registration::new(&sharing.active_tasks);

            if initial_fn.is_some() { // could be an if let
                let mut f = initial_fn.unwrap(); 
                f();
            }
            // if let mut Some(f) = initial_fn { f(); }

          loop {
            // receive tasks, and run them
            // if task queue is empty, wait or, if we've got plenty of threads, terminate
                let mut task: Box<FnMut() + Send> = {
                    // this todo is a MutexGuard on the task queue
                    let mut todo = sharing.todo.lock().unwrap();

                    let task; // the task we hope to obtain
                    loop { // until we get a task
                        if let Some(poped_task) = todo.pop_front() {
                            task = poped_task; // we got one!
                            break;
                        }
                        // we are a new idle worker, so increment waiting_worker_count
                        // while we're in this loop
                        let _waiting_guard = Registration::new(&sharing.waiting_tasks);

                        // received only means we received a notify (on the condition variable)
                        // we should loop around and see if we can be the lucky winner of a task
                        // todo is the mutex guard returned from the wait on the condition variable
                        let received = if sharing.active_tasks.load(Ordering::Acquire)
                                                <= MIN_THREADS
                        {   // we will wait as long as necessary
                            // do we really need to reassign our MutexGuard??
                            todo = sharing.condvar.wait(todo).unwrap();
                            true

                        } else { // we might be redundant, wait only briefly
                            let (new_lock, waitres) = sharing.condvar
                                                             .wait_timeout(todo, IDLE_TIME)
                                                             .unwrap();
                            // do we really need to reassign our MutexGuard??
                            todo = new_lock;
                            !waitres.timed_out()
                        };

                        if !received && todo.is_empty() {
                            return; // redundant thread dies
                        }
                    }

                    task
                };

                task();
            }
        });
    }
}

impl Drop for TaskPool {
    fn drop(&mut self) {
      // make the active_worker_count greater than
      // MIN_THREADS + total workers do that they'll start shutting down
      // redundant threads and they'll keep shutting down until they all do!
      // Better to add a boolean shutdown flag to the Sharing structure!!
        self.sharing.active_tasks.store(999999999, Ordering::Release);
        self.sharing.condvar.notify_all();
    }
}

/*
  Proposed new design:
  A dispatcher sends tasks to workers through a MessageQueue 

  worker loops on
    receive an Option<Task> -- blocks!
    None -> terminates
    Some(task) -> calls it

  dispatcher operations
    fn add_task(thunk):
      (1) adjust number of threads
          creating one if too few (<MIN and >MAX)
          send None if too many
      (2) sends a Some(task) into the Message Queue
    fn shutdown: sends None until all workers terminate

  MIN and MAX should come from toml file!
*/
