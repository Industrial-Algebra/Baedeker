;; Control flow: br_if execution cohort.

(module
  ;; br_if not taken falls through to the next instruction.
  ;; br_if taken skips to the end of the enclosing block.
  (func (export "br_if_skip") (param i32) (result i32)
    block
      local.get 0
      br_if 0
      i32.const 10
      return
    end
    i32.const 20)

  ;; return inside a block must not stop lowering of the code after
  ;; the block's end (regression: lowering used to halt at return).
  (func (export "return_in_block") (param i32) (result i32)
    block
      local.get 0
      br_if 0
      i32.const 10
      return
    end
    i32.const 20)

  ;; br_if carrying a block-result value: taken path delivers the
  ;; value to the continuation; not-taken path keeps it on the stack.
  (func (export "br_if_value") (param i32) (result i32)
    block (result i32)
      i32.const 7
      local.get 0
      br_if 0
      drop
      i32.const 9
    end)

  ;; br_if to an outer label from inside a nested block.
  (func (export "br_if_outer") (param i32) (result i32)
    block (result i32)
      block
        i32.const 1
        local.get 0
        br_if 1
        drop
      end
      i32.const 2
    end)
)

(assert_return (invoke "br_if_skip" (i32.const 0)) (i32.const 10))
(assert_return (invoke "br_if_skip" (i32.const 1)) (i32.const 20))
(assert_return (invoke "return_in_block" (i32.const 0)) (i32.const 10))
(assert_return (invoke "return_in_block" (i32.const 1)) (i32.const 20))
(assert_return (invoke "br_if_value" (i32.const 1)) (i32.const 7))
(assert_return (invoke "br_if_value" (i32.const 0)) (i32.const 9))
(assert_return (invoke "br_if_outer" (i32.const 1)) (i32.const 1))
(assert_return (invoke "br_if_outer" (i32.const 0)) (i32.const 2))
