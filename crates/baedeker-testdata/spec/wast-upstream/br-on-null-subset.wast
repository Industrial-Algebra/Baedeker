;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/br_on_null.wast

(module
  (type $t (func (result i32)))

  (func $nn (param $r (ref $t)) (result i32)
    (block $l
      (return (call_ref $t (br_on_null $l (local.get $r)))))
    (i32.const -1))

  (func $n (param $r (ref null $t)) (result i32)
    (block $l
      (return (call_ref $t (br_on_null $l (local.get $r)))))
    (i32.const -1))

  (elem func $f)
  (func $f (result i32) (i32.const 7)))

(module
  (type $t (func))
  (func (param $r (ref null $t)) (drop (br_on_null 0 (local.get $r))))
  (func (param $r (ref null func)) (drop (br_on_null 0 (local.get $r))))
  (func (param $r (ref null extern)) (drop (br_on_null 0 (local.get $r)))))

(module
  (type $t (func (param i32) (result i32)))
  (elem func $f)
  (func $f (param i32) (result i32) (i32.mul (local.get 0) (local.get 0)))

  (func $a (param $n i32) (param $r (ref null $t)) (result i32)
    (block $l (result i32)
      (return (call_ref $t (br_on_null $l (local.get $n) (local.get $r)))))))

(assert_invalid
  (module
    (type $t (func))
    (func $f (param (ref null $t)) (result funcref) (local.get 0))
    (func (param funcref) (result funcref)
      (ref.null $t)
      (local.get 0)
      (br_on_null 0)
      (drop)
      (call $f)))
  "type mismatch"
)
