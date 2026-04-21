;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/local_tee.wast

(module
  (func (export "type-local-i32") (result i32) (local i32) (local.tee 0 (i32.const 0)))
  (func (export "type-param-i64") (param i64) (result i64) (local.tee 0 (i64.const 11)))
  (func (export "type-mixed") (param i64 f32 f64 i32 i32) (local f32 i64 i64 f64)
    (drop (i64.eqz (local.tee 0 (i64.const 0))))
    (drop (f32.neg (local.tee 1 (f32.const 0))))
    (drop (f64.neg (local.tee 2 (f64.const 0))))
    (drop (i32.eqz (local.tee 3 (i32.const 0))))
    (drop (i32.eqz (local.tee 4 (i32.const 0))))
    (drop (f32.neg (local.tee 5 (f32.const 0))))
    (drop (i64.eqz (local.tee 6 (i64.const 0))))
    (drop (i64.eqz (local.tee 7 (i64.const 0))))
    (drop (f64.neg (local.tee 8 (f64.const 0))))))

(assert_invalid
  (module (func $type-local-num-vs-num (result i64) (local i32) (local.tee 0 (i32.const 0))))
  "type mismatch"
)

(assert_invalid
  (module (func $unbound-local (local i32 i64) (local.tee 3 (i32.const 0)) drop))
  "unknown local"
)
