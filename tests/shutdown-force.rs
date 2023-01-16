use std::time::Duration;

use unblock::unblock;

#[test]
fn test_shutdown_f() {
    #[allow(unused_must_use)]
    std::thread::spawn(|| loop {
        unblock(|| std::thread::sleep(Duration::from_secs(1)));
    });
    std::thread::sleep(Duration::from_millis(100))
}
