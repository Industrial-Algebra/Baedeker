;; Source fragments:
;; - https://github.com/WebAssembly/spec/blob/main/test/core/return_call_ref.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/ref_null.wast

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i64) (result i32)))

  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))

  (func (param $x i32) (result i32)
    (return_call_ref $t0 (local.get $x) (ref.func $f)))

  (func (param $x i32) (result i32)
    (return_call_ref $t0 (local.get $x) (ref.null $t0))))

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i32) (result i32)))

  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))

  (func (param $x i32) (result i32)
    (return_call_ref $t1 (local.get $x) (ref.func $f))))

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i32) (result i32)))

  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))

  (func (param $cond i32) (param $x i32) (result i32)
    (return_call_ref $t1
      (local.get $x)
      (if (result (ref null $t0))
        (local.get $cond)
        (then (ref.func $f))
        (else (ref.null $t0))))))

(module
  (type $i64-i64 (func (param i64) (result i64)))

  (elem declare func $count)
  (global $count (ref $i64-i64) (ref.func $count))

  (func $count (export "count") (param i64) (result i64)
    (if (result i64) (i64.eqz (local.get 0))
      (then (local.get 0))
      (else
        (return_call_ref $i64-i64
          (i64.sub (local.get 0) (i64.const 1))
          (global.get $count))))))

(module
  (type $i64-i64 (func (param i64) (result i64)))

  (global $even (ref $i64-i64) (ref.func $even))
  (global $odd (ref $i64-i64) (ref.func $odd))

  (elem declare func $even)
  (func $even (export "even") (param i64) (result i64)
    (if (result i64) (i64.eqz (local.get 0))
      (then (i64.const 44))
      (else
        (return_call_ref $i64-i64
          (i64.sub (local.get 0) (i64.const 1))
          (global.get $odd)))))

  (elem declare func $odd)
  (func $odd (export "odd") (param i64) (result i64)
    (if (result i64) (i64.eqz (local.get 0))
      (then (i64.const 99))
      (else
        (return_call_ref $i64-i64
          (i64.sub (local.get 0) (i64.const 1))
          (global.get $even))))))

(assert_invalid
  (module
    (type $t0 (func (param i32) (result i32)))
    (type $t1 (func (param i64) (result i32)))
    (func $f (type $t0) (local.get 0))
    (export "f" (func $f))
    (func (param $x i64) (result i32)
      (return_call_ref $t1 (local.get $x) (ref.func $f))))
  "type mismatch"
)

(assert_invalid
  (module
    (type $t0 (func (param i32) (result i32)))
    (type $t1 (func (param i64) (result i32)))
    (func $f (type $t0)
      (local.get 0))
    (export "f" (func $f))
    (func (param $cond i32) (param $x i64) (result i32)
      (return_call_ref $t1
        (local.get $x)
        (if (result (ref null $t0))
          (local.get $cond)
          (then (ref.func $f))
          (else (ref.null $t0))))))
  "type mismatch"
)
