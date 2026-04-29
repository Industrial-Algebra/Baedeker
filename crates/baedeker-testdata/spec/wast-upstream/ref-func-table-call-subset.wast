;; Sources:
;; - https://github.com/WebAssembly/spec/blob/main/test/core/ref_func.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/call_indirect.wast

(module
  (type $sig (func (param i32) (result i32)))

  (func $f (type $sig) (local.get 0))
  (func $g (type $sig) (i32.add (local.get 0) (i32.const 1)))

  (global funcref (ref.func $f))
  (global $v (mut funcref) (ref.func $f))

  (elem declare func $f $g)

  (table $t0 1 funcref)
  (table $t1 1 funcref)
  (elem (table $t1) (i32.const 0) func $g)

  (func (export "call-f") (param $x i32) (result i32)
    (table.set $t0 (i32.const 0) (ref.func $f))
    (call_indirect $t0 (type $sig) (local.get $x) (i32.const 0)))

  (func (export "call-g-through-get") (param $x i32) (result i32)
    (table.set $t0 (i32.const 0) (table.get $t1 (i32.const 0)))
    (call_indirect $t0 (type $sig) (local.get $x) (i32.const 0)))

  (func (export "call-v") (param $x i32) (result i32)
    (table.set $t0 (i32.const 0) (global.get $v))
    (call_indirect $t0 (type $sig) (local.get $x) (i32.const 0)))

  (func (export "is-null-v") (result i32)
    (ref.is_null (global.get $v))))

(assert_invalid
  (module
    (type $sig (func (param i32) (result i32)))
    (func $f (type $sig) (local.get 0))
    (table 1 externref)
    (elem declare func $f)
    (func
      (table.set 0 (i32.const 0) (ref.func $f))))
  "type mismatch"
)

(assert_invalid
  (module
    (type $sig (func (param i32) (result i32)))
    (func $f (type $sig) (local.get 0))
    (table $t 1 funcref)
    (elem declare func $f)
    (func (result i32)
      (call_indirect $t (type $sig) (ref.func $f) (i32.const 0))))
  "type mismatch"
)
