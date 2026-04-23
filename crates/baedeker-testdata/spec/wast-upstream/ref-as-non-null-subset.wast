;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/ref_as_non_null.wast

(module
  (type $t (func (result i32)))

  (func (param $r (ref $t)) (result i32)
    (call_ref $t (ref.as_non_null (local.get $r))))

  (func (param $r (ref null $t)) (result i32)
    (call_ref $t (ref.as_non_null (local.get $r)))))

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
  (type $t (func))
  (func (param $r (ref $t))
    (drop (ref.as_non_null (local.get $r))))
  (func (param $r (ref func))
    (drop (ref.as_non_null (local.get $r))))
  (func (param $r (ref extern))
    (drop (ref.as_non_null (local.get $r)))))
