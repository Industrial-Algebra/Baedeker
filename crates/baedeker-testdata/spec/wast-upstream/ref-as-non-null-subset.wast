;; Source fragments:
;; - https://github.com/WebAssembly/spec/blob/main/test/core/ref_as_non_null.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/ref_null.wast

(module
  (type $t (func (result i32)))

  (func (param $r (ref $t)) (result i32)
    (call_ref $t (ref.as_non_null (local.get $r))))

  (func (param $r (ref null $t)) (result i32)
    (call_ref $t (ref.as_non_null (local.get $r)))))

(module
  (type $t (func (result i32)))
  (func $nn (param $r (ref $t)) (result i32)
    (call_ref $t (ref.as_non_null (local.get $r))))
  (elem func $f)
  (func $f (result i32) (i32.const 7))
  (func (export "unreachable") (result i32)
    (unreachable)
    (ref.as_non_null)
    (call $nn)))

(assert_invalid
  (module
    (type $t (func (result i32)))
    (func $g (param $r (ref $t))
      (drop (ref.as_non_null (local.get $r))))
    (func
      (call $g (ref.null $t))))
  "type mismatch"
)

(module
  (type $t0 (func (param i32) (result i32)))
  (type $t1 (func (param i32) (result i32)))
  (import "env" "g" (global (mut (ref null $t1))))
  (func (param $r (ref null $t0))
    (global.set 0 (ref.as_non_null (local.get $r)))))

(assert_invalid
  (module
    (type $t0 (func (param i32) (result i32)))
    (type $t1 (func (param i64) (result i32)))
    (import "env" "g" (global (mut (ref null $t1))))
    (func (param $r (ref null $t0))
      (global.set 0 (ref.as_non_null (local.get $r)))))
  "type mismatch"
)

(module
  (type $t (func))
  (func (param $r (ref $t))
    (drop (ref.as_non_null (local.get $r))))
  (func (param $r (ref func))
    (drop (ref.as_non_null (local.get $r))))
  (func (param $r (ref extern))
    (drop (ref.as_non_null (local.get $r)))))
