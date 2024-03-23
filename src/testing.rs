use std::cell::RefCell;
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
