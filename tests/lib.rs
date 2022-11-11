use std::time::Duration;

use futures::executor::block_on;
use futures::future::{join, join_all};

use unblock::{unblock, unblocks};

macro_rules! test {
    ($name:ident -> $block:block) => {
        #[test]
        fn $name() {
            block_on(async { $block })
        }
    };
}

test!(test_sleep -> {
    assert_eq!(
        unblock(|| {
            std::thread::sleep(std::time::Duration::from_secs(1));
            "foo"
        })
        .await
        .unwrap(),
        "foo"
    )
});

fn sleep() {
    std::thread::sleep(Duration::from_millis(1));
}

test!(test_join -> {
    let mut fut = Vec::with_capacity(256);
    for _ in 0..256 {
        fut.push(unblock(sleep))
    }
    assert!(join_all(fut).await.iter().all(|x| x.is_ok()));
});

// #[cfg(not(miri))]
test!(test_panic -> {
    assert!(unblock(|| {
        panic!();
    })
    .await
    .is_err());
    assert_eq!(
        unblock(|| {
            std::thread::sleep(std::time::Duration::from_secs(1));
            "foo"
        })
        .await
        .unwrap(),
        "foo"
    );
    assert!(
        unblock(|| {
            std::thread::current().name().map(|x| x.to_string())
        })
        .await
        .unwrap()
        .unwrap()
        .starts_with("unblock")
    );
});

#[cfg(not(miri))]
test!(test_unblocks -> {
    for (x, i) in unblocks((0..10).map(|x| {
        move || {
            sleep();
            x
        }
    }))
    .into_iter()
    .enumerate()
    {
        assert_eq!(i.await, Ok(x));
    }
});

#[test]
fn test_thread() {
    let j = std::thread::spawn(|| block_on(unblock(sleep)));
    let fut_j = std::thread::spawn(|| unblock(sleep)).join().unwrap();
    assert!(j.join().is_ok());
    let (i, j) = block_on(join(unblock(sleep), fut_j));
    assert!(i.is_ok());
    assert!(j.is_ok());
}

#[test]
fn test_shutdown() {
    #[allow(unused_must_use)]
    std::thread::spawn(|| {
        for _ in 0..1024 * 1024 * 1024 {
            std::thread::sleep(Duration::from_millis(1));
            unblock(sleep);
        }
    });
    std::thread::sleep(Duration::from_millis(100));
    assert!(block_on(unblock(sleep)).is_ok());
}
