;; Control flow: block and br execution cohort.

(module
  ;; Simple empty block
  (func (export "empty_block") (result i32)
    block
    end
    i32.const 42)

  ;; br 0 skips, then produces result
  (func (export "br_skip") (result i32)
    block
      br 0
    end
    i32.const 99)

  ;; Multiple branches skip to end
  (func (export "br_skip_chain") (result i32)
    block
      block
        br 1
      end
    end
    i32.const 7)

  ;; br carrying a block-result value to the continuation.
  (func (export "br_value") (result i32)
    block (result i32)
      i32.const 1
      br 0
      i32.const 2
    end)

  ;; br to an outer block carrying a value from a nested block.
  (func (export "br_value_outer") (result i32)
    block (result i32)
      block
        i32.const 3
        br 1
      end
      i32.const 4
    end)
  ;; br from a nested unreachable region; the continuation must still
  ;; receive the outer block's value from the branch site.
  (func (export "br_value_poly") (result i32)
    block (result i32)
      i32.const 5
      br 0
      i32.const 6
      drop
    end))

(assert_return (invoke "empty_block") (i32.const 42))
(assert_return (invoke "br_skip") (i32.const 99))
(assert_return (invoke "br_skip_chain") (i32.const 7))
(assert_return (invoke "br_value") (i32.const 1))
(assert_return (invoke "br_value_outer") (i32.const 3))
(assert_return (invoke "br_value_poly") (i32.const 5))
