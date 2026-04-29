;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/local_set.wast

(module
  (func (export "type-local-i32") (local i32) (local.set 0 (i32.const 0)))
  (func (export "type-param-f64") (param f64) (local.set 0 (f64.const 12.2)))
  (func (export "type-mixed") (param i64 f32 f64 i32 i32) (local f32 i64 i64 f64)
    (local.set 0 (i64.const 0))
    (local.set 1 (f32.const 0))
    (local.set 2 (f64.const 0))
    (local.set 3 (i32.const 0))
    (local.set 4 (i32.const 0))
    (local.set 5 (f32.const 0))
    (local.set 6 (i64.const 0))
    (local.set 7 (i64.const 0))
    (local.set 8 (f64.const 0))))

(module
  (type $t0 (func (param i32) (result i32)))
  (func $f (type $t0)
    (local.get 0))
  (export "f" (func $f))
  (func (param $c i32)
    (local $r (ref null $t0))
    (local.set $r
      (if (result (ref null $t0))
        (local.get $c)
        (then
          (ref.func $f))
        (else
          (ref.null $t0))))))

(assert_invalid
  (module (func $type-local-arg-void-vs-num (local i32) (local.set 0 (nop))))
  "type mismatch"
)

(assert_invalid
  (module
    (type $t0 (func (param i32) (result i32)))
    (func $f (type $t0)
      (local.get 0))
    (export "f" (func $f))
    (func (param $c i32)
      (local $r (ref $t0))
      (local.set $r
        (if (result (ref null $t0))
          (local.get $c)
          (then
            (ref.func $f))
          (else
            (ref.null $t0))))))
  "type mismatch"
)

(assert_invalid
  (module (func $unbound-local (local i32 i64) (local.set 3 (i32.const 0))))
  "unknown local"
)
