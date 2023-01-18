# unblock [![Latest version](https://img.shields.io/crates/v/unblock.svg)](https://crates.io/crates/unblock)
A thread pool for isolating blocking in async programs.

With `mt` feature the default number of threads (set to number of cpus) can be altered 
by setting `BLOCK_THREADS` environment variable with value.

`kanal` and `async-oneshot` features are based in possible unsafe or deadlock implementations. 
Use under your responsibility.

## License

Licensed under either of

 * Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

Fork from [blocking](https://github.com/smol-rs/blocking)

#### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
