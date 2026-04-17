;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/global.wast

(module
  (global (mut f32) (f32.const 0))
  (export "a" (global 0))
)

(assert_invalid
  (module (global f32 (f32.const 0)) (func (global.set 0 (f32.const 1))))
  "immutable global"
)

(assert_invalid
  (module (global f32 (f32.neg (f32.const 0))))
  "constant expression required"
)

(assert_invalid
  (module (global f32 (local.get 0)))
  "constant expression required"
)
