;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/store.wast

(module
  (memory 1)

  (func (export "as-block-value")
    (block (i32.store (i32.const 0) (i32.const 1)))
  )
  (func (export "as-loop-value")
    (loop (i32.store (i32.const 0) (i32.const 1)))
  )

  (func (export "as-br-value")
    (block (br 0 (i32.store (i32.const 0) (i32.const 1))))
  ))

(assert_invalid
  (module (memory 1) (func (param i32) (result i32) (i32.store (i32.const 0) (i32.const 1))))
  "type mismatch"
)

(assert_invalid
  (module
    (memory 1)
    (func $type-address-empty
      (i32.store)))
  "type mismatch"
)

(assert_invalid
  (module (memory 1) (func (i32.store (i32.const 0) (f32.const 0))))
  "type mismatch"
)
