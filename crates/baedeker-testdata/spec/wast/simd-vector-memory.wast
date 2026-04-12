(module
  (memory 1)
  (func (result v128)
    i32.const 0
    v128.load)
  (func
    i32.const 0
    v128.const i32x4 0 0 0 0
    v128.store)
  (func (result v128)
    i32.const 0
    v128.const i32x4 0 0 0 0
    v128.load16_lane 7)
  (func
    i32.const 0
    v128.const i32x4 0 0 0 0
    v128.store32_lane 3))

(assert_invalid
  (module
    (memory 1)
    (func (result v128)
      i32.const 0
      v128.load align=32))
  "alignment")

(assert_invalid
  (module
    (memory 1)
    (func (result v128)
      i32.const 0
      v128.const i32x4 0 0 0 0
      v128.load16_lane 8))
  "lane")

(assert_invalid
  (module
    (memory 1)
    (func
      i32.const 0
      v128.const i32x4 0 0 0 0
      v128.store32_lane 4))
  "lane")

(assert_invalid
  (module
    (memory 1)
    (func (result v128)
      i32.const 0
      i32.const 0
      v128.load16_lane 0))
  "type mismatch")
