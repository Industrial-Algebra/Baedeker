;; Source fragments:
;; - https://github.com/WebAssembly/spec/blob/main/test/core/return_call_indirect.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/ref_null.wast

(module
  (type $out-i32 (func (result i32)))
  (type $over-i32 (func (param i32) (result i32)))

  (func $const-i32 (type $out-i32) (i32.const 0))
  (func $id-i32 (type $over-i32) (local.get 0))

  (table funcref (elem $const-i32 $id-i32))

  (func (result i32)
    (return_call_indirect (type $out-i32) (i32.const 0)))

  (func (result i32)
    (return_call_indirect (type $over-i32) (i32.const 7) (i32.const 1))))

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i32) (result i32)))
  (type $callee (func (param (ref null $t1)) (result i32)))

  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))

  (func $use (type $callee)
    (local.get 0)
    drop
    (i32.const 1))

  (table funcref (elem $use))

  (func (param $cond i32) (result i32)
    (return_call_indirect (type $callee)
      (if (result (ref null $t0))
        (local.get $cond)
        (then (ref.func $f))
        (else (ref.null $t0)))
      (i32.const 0))))

(module
  (type $t0 (func (param i32) (result i32)))
  (type $callee (func (result (ref null $t0))))
  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))
  (func $g (type $callee)
    (ref.null $t0))
  (table $ft 1 funcref)
  (elem funcref
    (ref.func $g))
  (func (result (ref null $t0))
    (table.init $ft 0 (i32.const 0) (i32.const 0) (i32.const 1))
    (return_call_indirect (type $callee)
      (i32.const 0))))

(assert_invalid
  (module
    (type $t0 (func (param i32) (result i32)))
    (type $t1 (func (param i64) (result i32)))
    (type $callee (func (param (ref null $t1)) (result i32)))
    (func $f (type $t0)
      (local.get 0))
    (export "f" (func $f))
    (func $use (type $callee)
      (local.get 0)
      drop
      (i32.const 1))
    (table funcref (elem $use))
    (func (param $cond i32) (result i32)
      (return_call_indirect (type $callee)
        (if (result (ref null $t0))
          (local.get $cond)
          (then (ref.func $f))
          (else (ref.null $t0)))
        (i32.const 0))))
  "type mismatch"
)

(assert_invalid
  (module
    (type $t0 (func (param i32) (result i32)))
    (type $callee (func (result (ref null $t0))))
    (func $f (type $t0)
      (local.get 0))
    (export "f" (func $f))
    (func $g (type $callee)
      (ref.null $t0))
    (table $ft 1 funcref)
    (elem funcref
      (ref.func $g))
    (func (result (ref $t0))
      (table.init $ft 0 (i32.const 0) (i32.const 0) (i32.const 1))
      (return_call_indirect (type $callee)
        (i32.const 0))))
  "type mismatch"
)

(assert_invalid
  (module
    (type $proc (func))
    (table 1 funcref)
    (func (result i32)
      (return_call_indirect (type $proc) (i32.const 0))))
  "type mismatch"
)

(assert_invalid
  (module
    (type $over-i32 (func (param i32) (result i32)))
    (table 1 funcref)
    (func (result i32)
      (return_call_indirect (type $over-i32) (i32.const 0))))
  "type mismatch"
)

(assert_invalid
  (module
    (type $out-i32 (func (result i32)))
    (table 1 externref)
    (func (result i32)
      (return_call_indirect (type $out-i32) (i32.const 0))))
  "type mismatch"
)
