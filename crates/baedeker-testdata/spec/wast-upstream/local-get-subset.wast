;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/local_get.wast

(module
  (func (export "type-local-i32") (result i32) (local i32) (local.get 0))
  (func (export "type-param-i64") (param i64) (result i64) (local.get 0))
  (func (export "type-mixed") (param i64 f32 f64 i32 i32)
    (local f32 i64 i64 f64)
    (drop (i64.eqz (local.get 0)))
    (drop (f32.neg (local.get 1)))
    (drop (f64.neg (local.get 2)))
    (drop (i32.eqz (local.get 3)))
    (drop (i32.eqz (local.get 4)))
    (drop (f32.neg (local.get 5)))
    (drop (i64.eqz (local.get 6)))
    (drop (i64.eqz (local.get 7)))
    (drop (f64.neg (local.get 8)))))

(assert_invalid
  (module (func $type-local-num-vs-num (result i64) (local i32) (local.get 0)))
  "type mismatch"
)

(assert_invalid
  (module (func $unbound-local (local i32 i64) (local.get 3) drop))
  "unknown local"
)
