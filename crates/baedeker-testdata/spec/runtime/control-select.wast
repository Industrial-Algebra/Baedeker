;; Control flow: select execution cohort.

(module
  ;; Untyped select on each numeric type.
  (func (export "select_i32") (param i32 i32 i32) (result i32)
    local.get 0
    local.get 1
    local.get 2
    select)

  (func (export "select_i64") (param i64 i64 i32) (result i64)
    local.get 0
    local.get 1
    local.get 2
    select)

  (func (export "select_f64") (param f64 f64 i32) (result f64)
    local.get 0
    local.get 1
    local.get 2
    select)

  ;; Typed select with an explicit result annotation.
  (func (export "select_typed") (param i32 i32 i32) (result i32)
    local.get 0
    local.get 1
    local.get 2
    select (result i32))

  ;; select computed from a comparison feeds control flow.
  (func (export "select_abs") (param i32) (result i32)
    local.get 0
    i32.const 0
    local.get 0
    i32.const 0
    i32.gt_s
    select
    local.get 0
    i32.sub)

  ;; select in dead code after return still lowers and executes the
  ;; live part correctly (polymorphic stack discipline).
  (func (export "select_dead") (result i32)
    i32.const 1
    i32.const 2
    i32.const 0
    select
    return
    i32.const 3
    i32.const 4
    i32.const 1
    select
    drop)
)

(assert_return (invoke "select_i32" (i32.const 10) (i32.const 20) (i32.const 1)) (i32.const 10))
(assert_return (invoke "select_i32" (i32.const 10) (i32.const 20) (i32.const 0)) (i32.const 20))
(assert_return (invoke "select_i64" (i64.const 7) (i64.const 9) (i32.const 1)) (i64.const 7))
(assert_return (invoke "select_i64" (i64.const 7) (i64.const 9) (i32.const 0)) (i64.const 9))
(assert_return (invoke "select_f64" (f64.const 1.5) (f64.const 2.5) (i32.const 1)) (f64.const 1.5))
(assert_return (invoke "select_f64" (f64.const 1.5) (f64.const 2.5) (i32.const 0)) (f64.const 2.5))
(assert_return (invoke "select_typed" (i32.const 3) (i32.const 4) (i32.const 1)) (i32.const 3))
(assert_return (invoke "select_typed" (i32.const 3) (i32.const 4) (i32.const 0)) (i32.const 4))
(assert_return (invoke "select_abs" (i32.const 5)) (i32.const 0))
(assert_return (invoke "select_abs" (i32.const -5)) (i32.const 5))
(assert_return (invoke "select_dead") (i32.const 2))
