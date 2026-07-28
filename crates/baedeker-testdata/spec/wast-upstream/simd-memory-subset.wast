;; Sources:
;; - https://github.com/WebAssembly/spec/blob/main/test/core/simd/simd_load.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/simd/simd_store.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/simd/simd_align.wast

(module
  (memory 1)
  (func (result v128)
    (v128.load (i32.const 0)))
  (func
    (v128.store (i32.const 0) (v128.const i32x4 0 1 2 3)))
  (func (result v128)
    (v128.load8x8_s align=8 (i32.const 0)))
  (func (result v128)
    (v128.load16x4_u align=8 (i32.const 0)))
  (func (result v128)
    (v128.load32x2_s align=8 (i32.const 0)))
  (func (result v128)
    (v128.load8_splat align=1 (i32.const 0)))
  (func (result v128)
    (v128.load16_splat align=2 (i32.const 0)))
  (func (result v128)
    (v128.load32_splat align=4 (i32.const 0)))
  (func (result v128)
    (v128.load64_splat align=8 (i32.const 0)))
  (func (result v128)
    (v128.load32_zero (i32.const 0)))
  (func (result v128)
    (v128.load64_zero (i32.const 0))))

(assert_invalid
  (module (memory 1) (func (local v128) (drop (v128.load (f32.const 0)))))
  "type mismatch"
)

(assert_invalid
  (module (memory 1) (func (v128.store (f32.const 0) (v128.const i32x4 0 0 0 0))))
  "type mismatch"
)

(assert_invalid
  (module (memory 1) (func (result v128) (v128.load8x8_s align=16 (i32.const 0))))
  "alignment must not be larger than natural"
)

(assert_invalid
  (module (memory 1) (func (result v128) (v128.load32_splat align=8 (i32.const 0))))
  "alignment must not be larger than natural"
)

(assert_invalid
  (module (memory 1) (func (result v128) (v128.load64_zero align=16 (i32.const 0))))
  "alignment must not be larger than natural"
)
