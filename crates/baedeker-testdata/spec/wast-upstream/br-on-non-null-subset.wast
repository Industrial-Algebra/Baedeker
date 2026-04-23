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

  (elem func $f)
  (func $f (result i32) (i32.const 7)))

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
