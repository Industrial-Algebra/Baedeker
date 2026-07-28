(module
  (memory 1)
  (func (result i32)
    i32.const 0
    i32.load)
  (func
    i32.const 0
    i32.const 1
    i32.store)
  (func (result i64)
    i32.const 0
    i64.load32_u)
  (func
    i32.const 0
    i64.const 1
    i64.store32)
  (func (result f32)
    i32.const 0
    f32.load)
  (func
    i32.const 0
    f64.const 0
    f64.store))

(module
  (memory 1)
  (memory 1)
  (func (result i32)
    i32.const 0
    i32.load 1)
  (func
    i32.const 0
    i32.const 1
    i32.store 1))

(assert_invalid
  (module
    (memory 1)
    (func (result i32)
      i64.const 0
      i32.load))
  "type mismatch")

(assert_invalid
  (module
    (memory 1)
    (func
      i32.const 0
      i64.const 1
      i32.store))
  "type mismatch")

(assert_invalid
  (module
    (memory 1)
    (func (result i32)
      i32.const 0
      i32.load align=8))
  "alignment")

(assert_invalid
  (module
    (memory 1)
    (func
      i32.const 0
      i64.const 1
      i64.store32 align=8))
  "alignment")

(assert_invalid
  (module
    (memory 1)
    (func (result i32)
      i32.const 0
      i32.load 1))
  "unknown memory")
