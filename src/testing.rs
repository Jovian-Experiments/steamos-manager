use anyhow::{anyhow, Result};
use std::cell::{Cell, RefCell};
use std::ffi::OsStr;
use std::path::Path;
use std::rc::Rc;
use tempfile::{tempdir, TempDir};

thread_local! {
    static TEST: RefCell<Option<Rc<Test>>> = RefCell::new(None);
}

pub fn start() -> TestHandle {
    TEST.with(|lock| {
        assert!(lock.borrow().as_ref().is_none());
        let test: Rc<Test> = Rc::new(Test {
            base: tempdir().expect("Couldn't create test directory"),
            process_cb: Cell::new(|_, _| Err(anyhow!("No current process_cb"))),
        });
        *lock.borrow_mut() = Some(test.clone());
        TestHandle { test }
    })
}

pub fn stop() {
    TEST.with(|lock| *lock.borrow_mut() = None);
}

pub fn current() -> Rc<Test> {
    TEST.with(|lock| lock.borrow().as_ref().unwrap().clone())
}

pub struct Test {
    base: TempDir,
    pub process_cb: Cell<fn(&str, &[&OsStr]) -> Result<(i32, String)>>,
}

pub struct TestHandle {
    pub test: Rc<Test>,
}

impl Test {
    pub fn path(&self) -> &Path {
        self.base.path()
    }
}

impl Drop for TestHandle {
    fn drop(&mut self) {
        stop();
    }
}
