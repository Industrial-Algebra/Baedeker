;; Multi-value block types: parameters and results on blocks, loops, ifs.

(module
  ;; Block with two params and two results.
  (func (export "block_multi") (param i32 i32) (result i32 i32)
    local.get 0
    local.get 1
    block (param i32 i32) (result i32 i32)
      ;; stack: [a, b]
      i32.add
      local.get 0
    end)

  ;; if with a parameter: both bodies start with the param on the stack.
  (func (export "if_params") (param i32 i32) (result i32)
    local.get 0
    local.get 1
    if (param i32) (result i32)
      ;; stack: [a]
      i32.const 10
      i32.add
    else
      ;; stack: [a] again (reset to frame entry)
      i32.const 100
      i32.add
    end)

  ;; Loop with params threaded on the stack: sum n + (n-1) + ... + 1.
  ;; The back-edge carries [acc+n, n-1] into the next iteration.
  (func (export "loop_param_sum") (param i32) (result i32)
    (local $n i32)
    i32.const 0
    local.get 0
    loop (param i32 i32) (result i32)
      ;; stack: [acc, n]
      local.set $n
      local.get $n
      i32.eqz
      if (param i32) (result i32)
        ;; stack: [acc] — done
      else
        ;; stack: [acc]
        local.get $n
        i32.add
        local.get $n
        i32.const 1
        i32.sub
        br 1
      end
    end)

  ;; Same sum, but the back-edge is a br_if (conditional): exercises
  ;; trampoline copies so a not-taken br_if does not clobber the loop's
  ;; live parameter registers.
  (func (export "loop_param_sum_brif") (param i32) (result i32)
    (local $n i32)
    i32.const 0
    local.get 0
    loop (param i32 i32) (result i32)
      ;; stack: [acc, n]
      local.set $n
      local.get $n
      i32.add
      local.get $n
      i32.const 1
      i32.sub
      local.tee $n
      local.get $n
      i32.const 1
      i32.ge_s
      br_if 0
      drop
    end)

  ;; br_table targeting a loop header (trampoline) and an exit block.
  (func (export "loop_param_table") (param i32) (result i32)
    (local $n i32)
    block (result i32 i32)
      i32.const 0
      local.get 0
      loop (param i32 i32) (result i32 i32)
        ;; stack: [acc, n]
        local.set $n
        local.get $n
        i32.add
        local.get $n
        i32.const 1
        i32.sub
        local.tee $n
        local.get $n
        i32.eqz
        br_table 0 1
      end
    end
    drop)

  ;; Two results from one block feeding two locals.
  (func (export "swap") (param i32 i32) (result i32)
    (local i32 i32)
    local.get 0
    local.get 1
    block (param i32 i32) (result i32 i32)
      ;; [a, b] -> [b, a]
      local.set 3
      local.set 2
      local.get 3
      local.get 2
    end
    ;; [b, a]
    local.set 3
    local.set 2
    local.get 3
    local.get 2
    i32.sub)
)

(assert_return (invoke "block_multi" (i32.const 3) (i32.const 4)) (i32.const 7) (i32.const 3))
(assert_return (invoke "if_params" (i32.const 5) (i32.const 1)) (i32.const 15))
(assert_return (invoke "if_params" (i32.const 5) (i32.const 0)) (i32.const 105))
(assert_return (invoke "loop_param_sum" (i32.const 0)) (i32.const 0))
(assert_return (invoke "loop_param_sum" (i32.const 5)) (i32.const 15))
(assert_return (invoke "loop_param_sum" (i32.const 10)) (i32.const 55))
(assert_return (invoke "loop_param_sum_brif" (i32.const 0)) (i32.const 0))
(assert_return (invoke "loop_param_sum_brif" (i32.const 5)) (i32.const 15))
(assert_return (invoke "loop_param_sum_brif" (i32.const 10)) (i32.const 55))
(assert_return (invoke "loop_param_table" (i32.const 5)) (i32.const 15))
(assert_return (invoke "swap" (i32.const 10) (i32.const 3)) (i32.const 7))
