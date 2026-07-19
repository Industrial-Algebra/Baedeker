;; Control flow: br_table execution cohort.

(module
  ;; Three-way dispatch to blocks at different depths, each carrying a
  ;; value into a distinct continuation.
  ;;   index 0 -> exit inner  -> 0 + 10 + 100 = 110
  ;;   index 1 -> exit middle -> 0 + 100      = 100
  ;;   index 2 (or default) -> exit outer     = 0
  (func (export "br_table_three_way") (param i32) (result i32)
    block (result i32)
      block (result i32)
        block (result i32)
          i32.const 0
          local.get 0
          br_table 0 1 2
        end
        ;; inner continuation (index 0)
        i32.const 10
        i32.add
        br 0
      end
      ;; middle continuation (index 1)
      i32.const 100
      i32.add
      br 0
    end)

  ;; Default target when the index is out of range, including a
  ;; negative index (read as a large u32).
  (func (export "br_table_default") (param i32) (result i32)
    block (result i32)
      block (result i32)
        i32.const 1
        local.get 0
        br_table 0 1
      end
      ;; inner continuation (index 0)
      i32.const 10
      i32.add
      br 0
    end)
  ;; index 0 -> 1 + 10 = 11; out-of-range/negative -> default -> 1

  ;; br_table targeting a loop header (back-edge) and an exit block.
  (func (export "br_table_loop") (param i32) (result i32)
    (local i32)
    block
      loop
        local.get 0
        i32.eqz
        br_if 1
        local.get 1
        i32.const 1
        i32.add
        local.set 1
        local.get 0
        i32.const 1
        i32.sub
        local.tee 0
        i32.eqz
        br_table 0 1
      end
    end
    local.get 1)

  ;; br_table in dead code after br still lowers correctly.
  (func (export "br_table_dead") (result i32)
    block (result i32)
      i32.const 42
      br 0
      i32.const 0
      i32.const 0
      br_table 0 0
    end)
)

(assert_return (invoke "br_table_three_way" (i32.const 0)) (i32.const 110))
(assert_return (invoke "br_table_three_way" (i32.const 1)) (i32.const 100))
(assert_return (invoke "br_table_three_way" (i32.const 2)) (i32.const 0))
(assert_return (invoke "br_table_three_way" (i32.const 7)) (i32.const 0))
(assert_return (invoke "br_table_default" (i32.const 0)) (i32.const 11))
(assert_return (invoke "br_table_default" (i32.const 1)) (i32.const 1))
(assert_return (invoke "br_table_default" (i32.const -1)) (i32.const 1))
(assert_return (invoke "br_table_loop" (i32.const 0)) (i32.const 0))
(assert_return (invoke "br_table_loop" (i32.const 5)) (i32.const 5))
(assert_return (invoke "br_table_dead") (i32.const 42))
