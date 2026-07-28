;; SIMD v128 core: const, splat, extract/replace lane, lane arithmetic,
;; bitwise, memory round-trip.

(module
  (memory 1)

  (func (export "const_i32x4") (result v128)
    v128.const i32x4 1 2 3 4)

  (func (export "splat_extract") (param i32) (result i32)
    local.get 0
    i32x4.splat
    i32x4.extract_lane 2)

  (func (export "replace") (param i32) (result i32)
    v128.const i32x4 0 0 0 0
    local.get 0
    i32x4.replace_lane 1
    i32x4.extract_lane 1)

  (func (export "i32x4_add") (result v128)
    v128.const i32x4 1 2 3 4
    v128.const i32x4 10 20 30 40
    i32x4.add)

  (func (export "i32x4_mul") (result v128)
    v128.const i32x4 2 3 4 5
    v128.const i32x4 10 10 10 10
    i32x4.mul)

  ;; Integer lanes wrap on overflow.
  (func (export "i32x4_wrap") (result v128)
    v128.const i32x4 2147483647 0 0 0
    v128.const i32x4 1 0 0 0
    i32x4.add)

  (func (export "f32x4_add") (result v128)
    v128.const f32x4 1.5 2.5 3.5 4.5
    v128.const f32x4 0.5 0.5 0.5 0.5
    f32x4.add)

  (func (export "f32x4_div") (result v128)
    v128.const f32x4 1.0 4.0 9.0 16.0
    v128.const f32x4 2.0 2.0 3.0 4.0
    f32x4.div)

  (func (export "i8x16_add") (result v128)
    v128.const i8x16 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16
    v128.const i8x16 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1 1
    i8x16.add)

  ;; i8 lanes wrap too.
  (func (export "i8x16_wrap") (result v128)
    v128.const i8x16 127 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0
    v128.const i8x16 1 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0
    i8x16.add)

  (func (export "i16x8_sub") (result v128)
    v128.const i16x8 10 20 30 40 50 60 70 80
    v128.const i16x8 1 2 3 4 5 6 7 8
    i16x8.sub)

  (func (export "i64x2_add") (result v128)
    v128.const i64x2 1000000000000 5
    v128.const i64x2 1 7
    i64x2.add)

  (func (export "f64x2_mul") (result v128)
    v128.const f64x2 3.0 2.5
    v128.const f64x2 2.0 4.0
    f64x2.mul)

  (func (export "bitwise_and") (result v128)
    v128.const i32x4 0xFF 0xF0 0x0F 0xAA
    v128.const i32x4 0x0F 0xF0 0xF0 0x55
    v128.and)

  (func (export "bitwise_not") (result v128)
    v128.const i8x16 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0
    v128.not)

  ;; v128 store/load through linear memory.
  (func (export "mem_roundtrip") (param i32) (result i32)
    local.get 0
    v128.const i32x4 11 22 33 44
    v128.store
    local.get 0
    v128.load
    i32x4.extract_lane 2)

  (func (export "f32_splat") (param f32) (result v128)
    local.get 0
    f32x4.splat)
)

(assert_return (invoke "const_i32x4") (v128.const i32x4 1 2 3 4))
(assert_return (invoke "splat_extract" (i32.const 42)) (i32.const 42))
(assert_return (invoke "replace" (i32.const 7)) (i32.const 7))
(assert_return (invoke "i32x4_add") (v128.const i32x4 11 22 33 44))
(assert_return (invoke "i32x4_mul") (v128.const i32x4 20 30 40 50))
(assert_return (invoke "i32x4_wrap") (v128.const i32x4 -2147483648 0 0 0))
(assert_return (invoke "f32x4_add") (v128.const f32x4 2.0 3.0 4.0 5.0))
(assert_return (invoke "f32x4_div") (v128.const f32x4 0.5 2.0 3.0 4.0))
(assert_return (invoke "i8x16_add") (v128.const i8x16 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17))
(assert_return (invoke "i8x16_wrap") (v128.const i8x16 -128 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0))
(assert_return (invoke "i16x8_sub") (v128.const i16x8 9 18 27 36 45 54 63 72))
(assert_return (invoke "i64x2_add") (v128.const i64x2 1000000000001 12))
(assert_return (invoke "f64x2_mul") (v128.const f64x2 6.0 10.0))
(assert_return (invoke "bitwise_and") (v128.const i32x4 0x0F 0xF0 0 0))
(assert_return (invoke "bitwise_not") (v128.const i8x16 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1 -1))
(assert_return (invoke "mem_roundtrip" (i32.const 16)) (i32.const 33))
(assert_return (invoke "f32_splat" (f32.const 2.5)) (v128.const f32x4 2.5 2.5 2.5 2.5))
