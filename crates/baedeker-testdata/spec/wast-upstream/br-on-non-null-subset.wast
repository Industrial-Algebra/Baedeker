;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/br_on_non_null.wast

(module
  (type $t (func (result i32)))

  (func $nn (param $r (ref $t)) (result i32)
    (call_ref $t
      (block $l (result (ref $t))
        (br_on_non_null $l (local.get $r))
        (return (i32.const -1)))))

  (func $n (param $r (ref null $t)) (result i32)
    (call_ref $t
      (block $l (result (ref $t))
        (br_on_non_null $l (local.get $r))
        (return (i32.const -1)))))

  (func $n2 (param $r (ref null $t)) (result i32)
    (call_ref $t
      (ref.as_non_null
        (block $l (result (ref null $t))
          (br_on_non_null $l (local.get $r))
          (return (i32.const -2))))))

  (elem func $f)
  (func $f (result i32) (i32.const 7))

  (func (export "nonnullable-f") (result i32) (call $nn (ref.func $f)))
  (func (export "nullable-null") (result i32) (call $n (ref.null $t)))
  (func (export "nullable-f") (result i32) (call $n (ref.func $f)))
  (func (export "nullable2-null") (result i32) (call $n2 (ref.null $t)))
  (func (export "nullable2-f") (result i32) (call $n2 (ref.func $f)))

  (func (export "unreachable") (result i32)
    (block $l (result (ref $t))
      (br_on_non_null $l (unreachable))
      (return (i32.const -1)))
    (call_ref $t)))

(module
  (type $t (func))
  (func (param $r (ref null $t))
    (drop (block (result (ref $t)) (br_on_non_null 0 (local.get $r)) (unreachable))))
  (func (param $r (ref null func))
    (drop (block (result (ref func)) (br_on_non_null 0 (local.get $r)) (unreachable))))
  (func (param $r (ref null extern))
    (drop (block (result (ref extern)) (br_on_non_null 0 (local.get $r)) (unreachable)))))

(module
  (type $t (func (param i32) (result i32)))
  (elem func $f)
  (func $f (param i32) (result i32) (i32.mul (local.get 0) (local.get 0)))

  (func $a (param $n i32) (param $r (ref null $t)) (result i32)
    (call_ref $t
      (block $l (result i32 (ref $t))
        (return (br_on_non_null $l (local.get $n) (local.get $r)))))))

(module
  (type $t0 (func (result i32)))
  (type $t1 (func (result i32)))
  (func $f (type $t0)
    (i32.const 7))
  (export "f" (func $f))
  (func (param $r (ref null $t0)) (result i32)
    (call_ref $t1
      (block $l (result (ref $t1))
        (br_on_non_null $l (local.get $r))
        (return (i32.const -1))))))

(assert_invalid
  (module
    (type $t0 (func (result i32)))
    (type $t1 (func (param i64) (result i32)))
    (func $f (type $t0)
      (i32.const 7))
    (export "f" (func $f))
    (func (param $r (ref null $t0)) (result i32)
      (call_ref $t1
        (block $l (result (ref $t1))
          (br_on_non_null $l (local.get $r))
          (return (i32.const -1))))))
  "type mismatch"
)

(assert_invalid
  (module
    (type $t (func))
    (func $f (param (ref null $t)) (result funcref) (local.get 0))
    (func (param funcref) (result funcref funcref)
      (ref.null $t)
      (local.get 0)
      (br_on_non_null 0)
      (call $f)
      (local.get 0)))
  "type mismatch"
)
