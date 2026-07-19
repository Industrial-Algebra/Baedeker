;; Calls: indirect call execution cohort.

(module
  (type $unop (func (param i32) (result i32)))
  (table 4 funcref)

  (func $inc (type $unop)
    local.get 0
    i32.const 1
    i32.add)
  (func $dec (type $unop)
    local.get 0
    i32.const 1
    i32.sub)
  (func $double (type $unop)
    local.get 0
    i32.const 2
    i32.mul)

  (elem (i32.const 0) $inc $dec $double)

  (func (export "apply") (param i32 i32) (result i32)
    local.get 1
    local.get 0
    call_indirect (type $unop))

  ;; Wrong signature at the target: type mismatch trap.
  (func $wrong (param i32 i32) (result i32)
    local.get 0
    local.get 1
    i32.add)
  (elem declare func $wrong)
  (func (export "seed_wrong")
    i32.const 3
    ref.func $wrong
    table.set)
)

(assert_return (invoke "apply" (i32.const 0) (i32.const 41)) (i32.const 42))
(assert_return (invoke "apply" (i32.const 1) (i32.const 43)) (i32.const 42))
(assert_return (invoke "apply" (i32.const 2) (i32.const 21)) (i32.const 42))
(assert_trap (invoke "apply" (i32.const 3) (i32.const 1)) "uninitialized element")
(assert_trap (invoke "apply" (i32.const 10) (i32.const 1)) "undefined element")
(assert_return (invoke "seed_wrong"))
(assert_trap (invoke "apply" (i32.const 3) (i32.const 1)) "indirect call type mismatch")

;; Structurally identical types at different type indices must match.
(module
  (type $a (func (result i32)))
  (type $b (func (result i32)))
  (table 1 funcref)
  (func $f (type $a) i32.const 42)
  (elem (i32.const 0) $f)
  (func (export "go") (result i32)
    i32.const 0
    call_indirect (type $b))
)

(assert_return (invoke "go") (i32.const 42))

;; Indirect recursion through the table.
(module
  (type $unop (func (param i32) (result i32)))
  (table 1 funcref)
  (func $count (type $unop)
    local.get 0
    i32.eqz
    if (result i32)
      i32.const 0
    else
      local.get 0
      i32.const 1
      i32.sub
      i32.const 0
      call_indirect (type $unop)
      i32.const 1
      i32.add
    end)
  (elem (i32.const 0) $count)
  (func (export "count") (param i32) (result i32)
    local.get 0
    i32.const 0
    call_indirect (type $unop))
)

(assert_return (invoke "count" (i32.const 0)) (i32.const 0))
(assert_return (invoke "count" (i32.const 5)) (i32.const 5))
