;; Sources:
;; - https://github.com/WebAssembly/spec/blob/main/test/core/f32_cmp.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/f64_cmp.wast
;; - https://github.com/WebAssembly/spec/blob/main/test/core/float_misc.wast

(module
  (func (result i32)
    (f32.eq (f32.const 0) (f32.const 0)))
  (func (result i32)
    (f32.lt (f32.const 0) (f32.const 1)))
  (func (result f32)
    (f32.abs (f32.const -1)))
  (func (result f32)
    (f32.sqrt (f32.const 4)))
  (func (result f32)
    (f32.copysign (f32.const 1) (f32.const -2)))
  (func (result f32)
    (f32.nearest (f32.const 1.5))))

(module
  (func (result i32)
    (f64.eq (f64.const 0) (f64.const 0)))
  (func (result i32)
    (f64.gt (f64.const 1) (f64.const 0)))
  (func (result f64)
    (f64.abs (f64.const -1)))
  (func (result f64)
    (f64.sqrt (f64.const 4)))
  (func (result f64)
    (f64.copysign (f64.const 1) (f64.const -2)))
  (func (result f64)
    (f64.nearest (f64.const 1.5))))

(assert_invalid
  (module
    (func (result i32)
      (f32.eq (i32.const 0) (f32.const 0))))
  "type mismatch"
)

(assert_invalid
  (module
    (func (result f32)
      (f32.sqrt (i32.const 4))))
  "type mismatch"
)

(assert_invalid
  (module
    (func (result f64)
      (f64.copysign (f64.const 1) (i32.const 0))))
  "type mismatch"
)
