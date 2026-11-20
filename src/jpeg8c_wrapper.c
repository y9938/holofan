#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <setjmp.h>

#include "jpeglib.h"

typedef struct {
  struct jpeg_error_mgr pub;
  jmp_buf jump;
  char message[JMSG_LENGTH_MAX];
  char warning[JMSG_LENGTH_MAX];
} hf_jpeg_error_mgr;

typedef struct {
  struct jpeg_decompress_struct cinfo;
  hf_jpeg_error_mgr error;
  uint8_t *output;
  int created;
} hf_jpeg_context;

static void hf_jpeg_error_exit(j_common_ptr cinfo) {
  hf_jpeg_error_mgr *err = (hf_jpeg_error_mgr *)cinfo->err;
  (*cinfo->err->format_message)(cinfo, err->message);
  longjmp(err->jump, 1);
}

static void hf_jpeg_emit_message(j_common_ptr cinfo, int level) {
  hf_jpeg_error_mgr *err = (hf_jpeg_error_mgr *)cinfo->err;
  if (level < 0 && err->warning[0] == '\0')
    (*cinfo->err->format_message)(cinfo, err->warning);
}

int hf_jpeg8c_decode_rgb(const uint8_t *input, size_t input_len,
                         uint8_t **pixels, uint32_t *width, uint32_t *height,
                         char *error, size_t error_cap,
                         char *warning, size_t warning_cap) {
  hf_jpeg_context *ctx;

  if (pixels) *pixels = NULL;
  if (width) *width = 0;
  if (height) *height = 0;
  if (error && error_cap) error[0] = '\0';
  if (warning && warning_cap) warning[0] = '\0';
  if (!input || input_len == 0 || !pixels || !width || !height) {
    if (error && error_cap) snprintf(error, error_cap, "invalid JPEG decoder arguments");
    return 0;
  }

  ctx = (hf_jpeg_context *)calloc(1, sizeof(*ctx));
  if (!ctx) {
    if (error && error_cap) snprintf(error, error_cap, "not enough memory for JPEG decoder");
    return 0;
  }

  ctx->cinfo.err = jpeg_std_error(&ctx->error.pub);
  ctx->error.pub.error_exit = hf_jpeg_error_exit;
  ctx->error.pub.emit_message = hf_jpeg_emit_message;

  if (setjmp(ctx->error.jump)) {
    free(ctx->output);
    if (ctx->created) jpeg_destroy_decompress(&ctx->cinfo);
    if (error && error_cap)
      snprintf(error, error_cap, "%s", ctx->error.message[0] ? ctx->error.message : "JPEG decode failed");
    free(ctx);
    return 0;
  }

  jpeg_create_decompress(&ctx->cinfo);
  ctx->created = 1;
  jpeg_mem_src(&ctx->cinfo, (const unsigned char *)input, (unsigned long)input_len);
  if (jpeg_read_header(&ctx->cinfo, TRUE) != JPEG_HEADER_OK) {
    snprintf(ctx->error.message, sizeof(ctx->error.message), "JPEG header is incomplete");
    longjmp(ctx->error.jump, 1);
  }

  ctx->cinfo.out_color_space = JCS_RGB;
  ctx->cinfo.quantize_colors = FALSE;
  if (!jpeg_start_decompress(&ctx->cinfo)) {
    snprintf(ctx->error.message, sizeof(ctx->error.message), "JPEG decompression did not start");
    longjmp(ctx->error.jump, 1);
  }
  if (ctx->cinfo.output_components != 3 || ctx->cinfo.output_width == 0 || ctx->cinfo.output_height == 0 ||
      (size_t)ctx->cinfo.output_width > SIZE_MAX / 3 / (size_t)ctx->cinfo.output_height) {
    snprintf(ctx->error.message, sizeof(ctx->error.message), "JPEG dimensions are invalid or too large");
    longjmp(ctx->error.jump, 1);
  }

  size_t row_bytes = (size_t)ctx->cinfo.output_width * 3;
  size_t total_bytes = row_bytes * (size_t)ctx->cinfo.output_height;
  ctx->output = (uint8_t *)malloc(total_bytes);
  if (!ctx->output) {
    snprintf(ctx->error.message, sizeof(ctx->error.message), "not enough memory for decoded JPEG");
    longjmp(ctx->error.jump, 1);
  }

  while (ctx->cinfo.output_scanline < ctx->cinfo.output_height) {
    JSAMPROW row = (JSAMPROW)(ctx->output + row_bytes * ctx->cinfo.output_scanline);
    if (jpeg_read_scanlines(&ctx->cinfo, &row, 1) != 1) {
      snprintf(ctx->error.message, sizeof(ctx->error.message), "JPEG scanline decode stopped early");
      longjmp(ctx->error.jump, 1);
    }
  }
  if (!jpeg_finish_decompress(&ctx->cinfo)) {
    snprintf(ctx->error.message, sizeof(ctx->error.message), "JPEG stream ended before decompression completed");
    longjmp(ctx->error.jump, 1);
  }

  *pixels = ctx->output;
  ctx->output = NULL;
  *width = ctx->cinfo.output_width;
  *height = ctx->cinfo.output_height;
  if (warning && warning_cap && ctx->error.warning[0])
    snprintf(warning, warning_cap, "%s", ctx->error.warning);
  jpeg_destroy_decompress(&ctx->cinfo);
  free(ctx);
  return 1;
}

void hf_jpeg8c_free(void *pixels) { free(pixels); }
