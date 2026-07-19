;; Tables: storage, growth, fill/copy/init, ref values.

(module
  (type $t (func (result i32)))
  (table $tbl 2 funcref)
  (func $ten (type $t) i32.const 10)
  (func $eleven (type $t) i32.const 11)
  (elem (i32.const 0) $ten $eleven)

  (func (export "size") (result i32)
    table.size $tbl)
  (func (export "grow") (param i32) (result i32)
    ref.null func
    local.get 0
    table.grow $tbl)
  (func (export "is_null_at") (param i32) (result i32)
    local.get 0
    table.get $tbl
    ref.is_null)
  (func (export "set_null") (param i32)
    local.get 0
    ref.null func
    table.set $tbl)
  (func (export "set_ten") (param i32)
    local.get 0
    ref.func $ten
    table.set $tbl)
  (func (export "call_at") (param i32) (result i32)
    local.get 0
    call_indirect $tbl (type $t))
)

(assert_return (invoke "size") (i32.const 2))
(assert_return (invoke "call_at" (i32.const 0)) (i32.const 10))
(assert_return (invoke "call_at" (i32.const 1)) (i32.const 11))
(assert_return (invoke "is_null_at" (i32.const 0)) (i32.const 0))

(assert_return (invoke "grow" (i32.const 3)) (i32.const 2))
(assert_return (invoke "size") (i32.const 5))
(assert_return (invoke "is_null_at" (i32.const 4)) (i32.const 1))

(assert_return (invoke "set_null" (i32.const 0)))
(assert_return (invoke "is_null_at" (i32.const 0)) (i32.const 1))
(assert_return (invoke "set_ten" (i32.const 0)))
(assert_return (invoke "is_null_at" (i32.const 0)) (i32.const 0))
(assert_return (invoke "call_at" (i32.const 0)) (i32.const 10))

;; table.get out of bounds traps.
(assert_trap (invoke "is_null_at" (i32.const 5)) "out of bounds table access")
(assert_trap (invoke "set_null" (i32.const 5)) "out of bounds table access")

;; Bounded growth: max 4 pages of table.
(module
  (table 2 4 funcref)
  (func (export "grow") (param i32) (result i32)
    ref.null func
    local.get 0
    table.grow)
  (func (export "size") (result i32)
    table.size)
)

(assert_return (invoke "size") (i32.const 2))
(assert_return (invoke "grow" (i32.const 2)) (i32.const 2))
(assert_return (invoke "grow" (i32.const 1)) (i32.const -1))
(assert_return (invoke "size") (i32.const 4))

;; table.init from a passive segment, table.copy, table.fill, elem.drop.
(module
  (type $t (func (result i32)))
  (table 4 funcref)
  (func $one (type $t) i32.const 1)
  (func $two (type $t) i32.const 2)
  (elem $p func $one $two)

  (func (export "call_at") (param i32) (result i32)
    local.get 0
    call_indirect (type $t))
  ;; table.init dst src count
  (func (export "init") (param i32 i32 i32)
    local.get 0
    local.get 1
    local.get 2
    table.init $p)
  (func (export "drop_elem")
    elem.drop $p)
  ;; table.copy dst src count
  (func (export "copy") (param i32 i32 i32)
    local.get 0
    local.get 1
    local.get 2
    table.copy)
  ;; table.fill dst val count
  (func (export "fill_ten") (param i32 i32)
    local.get 0
    ref.func $one
    local.get 1
    table.fill)
  (func (export "null_at") (param i32) (result i32)
    local.get 0
    table.get
    ref.is_null)
)

(assert_return (invoke "null_at" (i32.const 0)) (i32.const 1))
(assert_return (invoke "init" (i32.const 0) (i32.const 0) (i32.const 2)))
(assert_return (invoke "call_at" (i32.const 0)) (i32.const 1))
(assert_return (invoke "call_at" (i32.const 1)) (i32.const 2))
(assert_return (invoke "copy" (i32.const 2) (i32.const 0) (i32.const 2)))
(assert_return (invoke "call_at" (i32.const 3)) (i32.const 2))
(assert_return (invoke "fill_ten" (i32.const 2) (i32.const 2)))
(assert_return (invoke "call_at" (i32.const 3)) (i32.const 1))
(assert_return (invoke "drop_elem"))
(assert_trap (invoke "init" (i32.const 0) (i32.const 0) (i32.const 1)) "out of bounds table access")
(assert_trap (invoke "copy" (i32.const 3) (i32.const 0) (i32.const 2)) "out of bounds table access")
(assert_trap (invoke "fill_ten" (i32.const 3) (i32.const 2)) "out of bounds table access")
