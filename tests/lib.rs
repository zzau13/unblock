use std::time::Duration;

use futures::future::join_all;

use unblock::unblock;

macro_rules! test {
    ($name:ident -> $block:block) => {
        #[test]
        fn $name() {
            futures::executor::block_on(async { $block })
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
