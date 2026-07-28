;; Sources:
;; - https://github.com/WebAssembly/spec/blob/main/test/core/i32.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/i64.wast

(module
  (func (result i32)
    (i32.add (i32.const 1) (i32.const 2)))
  (func (result i32)
    (i32.clz (i32.const 1)))
  (func (result i32)
    (i32.popcnt (i32.const 7)))
  (func (result i32)
    (i32.extend8_s (i32.const 255)))
  (func (result i32)
    (i32.extend16_s (i32.const 65535)))
  (func (result i32)
    (i32.eqz (i32.const 0))))

(module
  (func (result i64)
    (i64.add (i64.const 1) (i64.const 2)))
  (func (result i64)
    (i64.ctz (i64.const 8)))
  (func (result i64)
    (i64.popcnt (i64.const 7)))
  (func (result i64)
    (i64.extend8_s (i64.const 255)))
  (func (result i64)
    (i64.extend16_s (i64.const 65535)))
  (func (result i64)
    (i64.extend32_s (i64.const 4294967295)))
  (func (result i32)
    (i64.eqz (i64.const 0))))

(assert_invalid
  (module
    (func (result i32)
      (i32.add (f32.const 0) (i32.const 1))))
  "type mismatch"
)

(assert_invalid
  (module
    (func (result i64)
      (i64.extend32_s (i32.const 0))))
  "type mismatch"
)

(assert_invalid
  (module
    (func (result i32)
      (i64.eqz (i32.const 0))))
  "type mismatch"
)
