;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/simd/simd_const.wast

(module
  (func (drop (v128.const i8x16 255 255 255 255 255 255 255 255 255 255 255 255 255 255 255 255)))
  (func (drop (v128.const i16x8 65535 65535 65535 65535 65535 65535 65535 65535)))
  (func (drop (v128.const i32x4 4294967295 4294967295 4294967295 4294967295)))
  (func (drop (v128.const i64x2 18446744073709551615 18446744073709551615)))
  (func (drop (v128.const f32x4 0 1 2 3)))
  (func (drop (v128.const f64x2 0 1))))

(assert_invalid
  (module
    (func (result i32)
      (v128.const i32x4 0 1 2 3)))
  "type mismatch"
)

(assert_invalid
  (module
    (func
      (v128.const i64x2 0 1)))
  "type mismatch"
)
