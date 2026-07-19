;; Calls: direct call execution cohort.

(module
  ;; Plain multi-argument call.
  (func $add (param i32 i32) (result i32)
    local.get 0
    local.get 1
    i32.add)
  (func (export "call_add") (param i32 i32) (result i32)
    local.get 0
    local.get 1
    call $add)

  ;; Direct recursion.
  (func $fac (param i32) (result i32)
    local.get 0
    i32.const 2
    i32.lt_s
    if (result i32)
      i32.const 1
    else
      local.get 0
      local.get 0
      i32.const 1
      i32.sub
      call $fac
      i32.mul
    end)
  (func (export "fac") (param i32) (result i32)
    local.get 0
    call $fac)

  ;; Two-way recursion.
  (func $fib (param i32) (result i32)
    local.get 0
    i32.const 2
    i32.lt_s
    if (result i32)
      local.get 0
    else
      local.get 0
      i32.const 1
      i32.sub
      call $fib
      local.get 0
      i32.const 2
      i32.sub
      call $fib
      i32.add
    end)
  (func (export "fib") (param i32) (result i32)
    local.get 0
    call $fib)

  ;; Mutual recursion across two functions.
  (func $is_even (param i32) (result i32)
    local.get 0
    i32.eqz
    if (result i32)
      i32.const 1
    else
      local.get 0
      i32.const 1
      i32.sub
      call $is_odd
    end)
  (func $is_odd (param i32) (result i32)
    local.get 0
    i32.eqz
    if (result i32)
      i32.const 0
    else
      local.get 0
      i32.const 1
      i32.sub
      call $is_even
    end)
  (func (export "even") (param i32) (result i32)
    local.get 0
    call $is_even)

  ;; Multi-result call: quotient and remainder.
  (func $divmod (param i32 i32) (result i32 i32)
    local.get 0
    local.get 1
    i32.div_u
    local.get 0
    local.get 1
    i32.rem_u)
  (func (export "divmod_q") (param i32 i32) (result i32)
    local.get 0
    local.get 1
    call $divmod
    drop)
  (func (export "divmod_r") (param i32 i32) (result i32)
    (local i32)
    local.get 0
    local.get 1
    call $divmod
    local.set 2
    drop
    local.get 2)

  ;; Unbounded recursion exhausts the call stack.
  (func $boom (export "boom")
    call $boom)
)

(assert_return (invoke "call_add" (i32.const 20) (i32.const 22)) (i32.const 42))
(assert_return (invoke "fac" (i32.const 0)) (i32.const 1))
(assert_return (invoke "fac" (i32.const 5)) (i32.const 120))
(assert_return (invoke "fib" (i32.const 0)) (i32.const 0))
(assert_return (invoke "fib" (i32.const 1)) (i32.const 1))
(assert_return (invoke "fib" (i32.const 10)) (i32.const 55))
(assert_return (invoke "even" (i32.const 10)) (i32.const 1))
(assert_return (invoke "even" (i32.const 7)) (i32.const 0))
(assert_return (invoke "divmod_q" (i32.const 17) (i32.const 5)) (i32.const 3))
(assert_return (invoke "divmod_r" (i32.const 17) (i32.const 5)) (i32.const 2))
(assert_exhaustion (invoke "boom") "call stack exhausted")
