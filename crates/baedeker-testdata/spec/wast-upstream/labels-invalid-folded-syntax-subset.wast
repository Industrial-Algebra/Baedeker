;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/labels.wast

(assert_invalid
  (module (func (block $l (f32.neg (br_if $l (i32.const 1))) (nop))))
  "type mismatch"
)

(assert_invalid
  (module (func (block $l (br_if $l (f32.const 0) (i32.const 1)))))
  "type mismatch"
)
