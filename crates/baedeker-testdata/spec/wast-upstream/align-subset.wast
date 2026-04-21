;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/align.wast

(module
  (memory 0)
  (func (drop (i32.load8_s align=1 (i32.const 0))))
  (func (drop (i64.load32_u align=4 (i32.const 0))))
  (func (f32.store align=4 (i32.const 0) (f32.const 1.0)))
  (func (i64.store align=1 (i32.const 0) (i64.const 1))))

(assert_malformed
  (module quote
    "(memory 0)"
    "(func (drop (i32.load align=0 (i32.const 0))))")
  "alignment"
)

(assert_malformed
  (module quote
    "(memory 0)"
    "(func (drop (f64.load align=7 (i32.const 0))))")
  "alignment"
)

(assert_invalid
  (module (memory 0) (func (drop (i32.load align=8 (i32.const 0)))))
  "alignment"
)

(assert_invalid
  (module (memory 0) (func (drop (i64.load32_u align=8 (i32.const 0)))))
  "alignment"
)

(assert_invalid
  (module (memory 0) (func (f64.store align=16 (i32.const 0) (f64.const 0))))
  "alignment"
)
