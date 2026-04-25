;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/global.wast

(module
  (global (import "spectest" "global_i32") i32)
  (global (import "spectest" "global_i64") i64)
  (global $a i32 (i32.const -2))
  (global $b i64 (i64.const -5))
  (global $z1 i32 (global.get 0))
  (global $z2 i64 (global.get 1))
  (global $z3 i32 (i32.add (i32.sub (i32.mul (i32.const 20) (i32.const 2)) (i32.const 2)) (i32.const 4)))
  (global $z4 i64 (i64.add (i64.sub (i64.mul (i64.const 20) (i64.const 2)) (i64.const 2)) (i64.const 5)))
  (global $z5 i32 (i32.add (global.get 0) (i32.const 42)))
  (global $z6 i64 (i64.add (global.get 1) (i64.const 42)))
  (global $r externref (ref.null extern))
  (global funcref (ref.null func)))

(module
  (global i32 (i32.const 0))
  (global i32 (global.get 0)))

(module
  (global $g i32 (i32.const 0))
  (global i32 (global.get $g)))

(module
  (global (mut f32) (f32.const 0))
  (export "a" (global 0)))

(assert_invalid
  (module (global f32 (f32.const 0)) (func (global.set 0 (f32.const 1))))
  "immutable global"
)

(assert_invalid
  (module (import "spectest" "global_i32" (global i32)) (func (global.set 0 (i32.const 1))))
  "immutable global"
)

(assert_invalid
  (module (global f32 (local.get 0)))
  "constant expression required"
)

(assert_invalid
  (module (global i32 (i32.ctz (i32.const 0))))
  "constant expression required"
)

(assert_invalid
  (module (global i32 (f32.const 0)))
  "type mismatch"
)

(assert_invalid
  (module (global (import "" "") externref) (global funcref (global.get 0)))
  "type mismatch"
)

(assert_invalid
  (module (global (import "test" "global-i32") i32) (global i32 (global.get 0) (global.get 0)))
  "type mismatch"
)
