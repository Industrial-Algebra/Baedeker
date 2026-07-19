;; Globals: const-init, mutation, chained init exprs.

(module
  (global $answer i32 (i32.const 42))
  (global $pi f64 (f64.const 3.5))
  (global $counter (mut i32) (i32.const 0))
  (global $big i64 (i64.const 0x100000000))
  ;; Init exprs may reference earlier globals (WASM 3.0).
  (global $derived i32 (global.get $answer))
  (global $extended i32 (i32.add (global.get $answer) (i32.const 8)))

  (func (export "get_answer") (result i32)
    global.get $answer)
  (func (export "get_pi") (result f64)
    global.get $pi)
  (func (export "get_big") (result i64)
    global.get $big)
  (func (export "get_derived") (result i32)
    global.get $derived)
  (func (export "get_extended") (result i32)
    global.get $extended)

  (func (export "bump") (result i32)
    global.get $counter
    i32.const 1
    i32.add
    global.set $counter
    global.get $counter)

  ;; Globals are visible across calls and mutate shared state.
  (func (export "add_to_counter") (param i32) (result i32)
    global.get $counter
    local.get 0
    i32.add
    global.set $counter
    global.get $counter)
)

(assert_return (invoke "get_answer") (i32.const 42))
(assert_return (invoke "get_pi") (f64.const 3.5))
(assert_return (invoke "get_big") (i64.const 0x100000000))
(assert_return (invoke "get_derived") (i32.const 42))
(assert_return (invoke "get_extended") (i32.const 50))
(assert_return (invoke "bump") (i32.const 1))
(assert_return (invoke "bump") (i32.const 2))
(assert_return (invoke "add_to_counter" (i32.const 5)) (i32.const 7))
(assert_return (invoke "bump") (i32.const 8))
