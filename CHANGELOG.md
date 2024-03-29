# 0.7.0 - 2023-01-16

    - Fix memory leaks when panics
    - Add `kanal` and `async-oneshot` features

# 0.6.0 - 2022-11-10

    - Remove panic unwind in favor of live monitor resurrector at panic

# 0.5.1 - 2022-11-10

    - Make sure force shutdown gracefully

# 0.5.0 - 2022-11-09

    - Add `lazy` feature for lazy init

# 0.4.1 - 2022-11-09

    - Improve shutdown and use only `ctor` for lazy static

# 0.4.0 - 2022-11-09

    - Add gracefully destroy executor and join in the main thread

# 0.3.1 - 2022-11-08

    - Simplify and prettify code and documentation

# 0.3.0 - 2022-11-08
    
    - Add `unblocks` for spawm multiple

# 0.2.0 - 2022-11-07

    - Improve performance removing unnecessary locks
    - Reduce size of Executor

# 0.1.0 - 2022-11-07

    - Initial release

# 0.0.1 - 2022-10-30

    - Reserved crate name
