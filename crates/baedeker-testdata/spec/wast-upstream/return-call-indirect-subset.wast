;; Source: https://github.com/WebAssembly/spec/blob/main/test/core/return_call_indirect.wast

(module
  (type $out-i32 (func (result i32)))
  (type $over-i32 (func (param i32) (result i32)))

  (func $const-i32 (type $out-i32) (i32.const 0))
  (func $id-i32 (type $over-i32) (local.get 0))

  (table funcref (elem $const-i32 $id-i32))

  (func (result i32)
    (return_call_indirect (type $out-i32) (i32.const 0)))

  (func (result i32)
    (return_call_indirect (type $over-i32) (i32.const 7) (i32.const 1))))

(assert_invalid
  (module
    (type $proc (func))
    (table 1 funcref)
    (func (result i32)
      (return_call_indirect (type $proc) (i32.const 0))))
  "type mismatch"
)

(assert_invalid
  (module
    (type $over-i32 (func (param i32) (result i32)))
    (table 1 funcref)
    (func (result i32)
      (return_call_indirect (type $over-i32) (i32.const 0))))
  "type mismatch"
)

(assert_invalid
  (module
    (type $out-i32 (func (result i32)))
    (table 1 externref)
    (func (result i32)
      (return_call_indirect (type $out-i32) (i32.const 0))))
  "type mismatch"
)
