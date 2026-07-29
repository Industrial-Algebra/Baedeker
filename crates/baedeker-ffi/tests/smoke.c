#include "baedeker.h"

#include <stdio.h>
#include <string.h>

/* A host function: multiply the two i32 args. */
static BaedekerStatus host_mul(const BaedekerValue *args,
                               size_t n_args,
                               BaedekerValue *results,
                               size_t n_results,
                               void *user_data) {
  (void)user_data;
  if (n_args != 2 || n_results != 1) {
    baedeker_set_last_error("bad host_mul arity");
    return BaedekerStatus_HostError;
  }
  results[0].tag = BaedekerValueTag_I32;
  results[0].data.i32_ = args[0].data.i32_ * args[1].data.i32_;
  return BaedekerStatus_Ok;
}

/* (module
     (import "env" "mul" (func $mul (param i32 i32) (result i32)))
     (memory (export "memory") 1)
     (func (export "mul6x7") (result i32)
       i32.const 6  i32.const 7  call $mul)) */
static const uint8_t MODULE[] = {
    0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x0b, 0x02, 0x60,
    0x02, 0x7f, 0x7f, 0x01, 0x7f, 0x60, 0x00, 0x01, 0x7f, 0x02, 0x0b, 0x01,
    0x03, 0x65, 0x6e, 0x76, 0x03, 0x6d, 0x75, 0x6c, 0x00, 0x00, 0x03, 0x02,
    0x01, 0x01, 0x05, 0x03, 0x01, 0x00, 0x01, 0x07, 0x13, 0x02, 0x06, 0x6d,
    0x65, 0x6d, 0x6f, 0x72, 0x79, 0x02, 0x00, 0x06, 0x6d, 0x75, 0x6c, 0x36,
    0x78, 0x37, 0x00, 0x01, 0x0a, 0x0a, 0x01, 0x08, 0x00, 0x41, 0x06, 0x41,
    0x07, 0x10, 0x00, 0x0b,
};

int main(void) {
  printf("baedeker %s\n", baedeker_version());

  BaedekerModule *module = NULL;
  BaedekerStatus status =
      baedeker_module_compile(MODULE, sizeof(MODULE), &module);
  if (status != BaedekerStatus_Ok) {
    fprintf(stderr, "compile failed: %d\n", status);
    return 1;
  }

  BaedekerInstance *instance = NULL;
  status = baedeker_instance_new(module, &instance);
  baedeker_module_free(module); /* instance keeps its own reference */
  if (status != BaedekerStatus_Ok) {
    fprintf(stderr, "instantiate failed: %d\n", status);
    return 1;
  }

  const uint8_t sig_params[] = {BaedekerValueTag_I32, BaedekerValueTag_I32};
  const uint8_t sig_results[] = {BaedekerValueTag_I32};
  status = baedeker_instance_register_host_func(
      instance, "env", "mul", sig_params, 2, sig_results, 1, host_mul, NULL);
  if (status != BaedekerStatus_Ok) {
    fprintf(stderr, "host registration failed: %d\n", status);
    return 1;
  }

  BaedekerValue result;
  size_t n_results = 0;
  status = baedeker_instance_call(instance, "mul6x7", NULL, 0, &result, 1,
                                  &n_results);
  if (status != BaedekerStatus_Ok || n_results != 1 ||
      result.data.i32_ != 42) {
    char buf[256];
    baedeker_last_error(buf, sizeof(buf));
    fprintf(stderr, "call failed: status=%d n=%zu value=%d msg=%s\n", status,
            n_results, result.data.i32_, buf);
    return 1;
  }
  printf("mul6x7() = %d\n", result.data.i32_);

  /* Memory poke/read-back through the copy API. */
  const uint8_t hello[] = {0xde, 0xad, 0xbe, 0xef};
  status = baedeker_instance_memory_write(instance, 64, hello, 4);
  uint8_t back[4] = {0};
  status |= baedeker_instance_memory_read(instance, 64, back, 4);
  if (status != BaedekerStatus_Ok || memcmp(hello, back, 4) != 0) {
    fprintf(stderr, "memory roundtrip failed\n");
    return 1;
  }
  printf("memory roundtrip ok\n");

  baedeker_instance_free(instance);
  printf("C end-to-end ok\n");
  return 0;
}
