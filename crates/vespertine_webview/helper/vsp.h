/* Shared declarations of the Vespertine WebKitGTK helper (PLT-014/015/016, helper side). */
#ifndef VSP_H
#define VSP_H

#include <gtk/gtk.h>
#include <json-glib/json-glib.h>
#include <stdint.h>
#include <webkit2/webkit2.h>

/* GObject signal and virtual-function callbacks have fixed signatures with parameters they ignore. */
#pragma GCC diagnostic ignored "-Wunused-parameter"

#define VSP_PROTOCOL_VERSION 1
#define VSP_HEADER_LEN 9
#define VSP_MAX_COMMAND_LEN (1024u * 1024u)
#define VSP_MAX_DIM 8192
#define VSP_MIN_SCALE 0.5
#define VSP_MAX_SCALE 4.0
#define VSP_DEFAULT_WIDTH 800
#define VSP_DEFAULT_HEIGHT 600
#define VSP_FRAME_TICK_MS 16
#define VSP_POOL_SLOTS 2
#define VSP_STRIDE_ALIGN 64

typedef struct Pool Pool;

typedef enum {
    TRANSFER_INLINE = 0,
    TRANSFER_SHM = 1,
} TransferMode;

typedef struct Page {
    guint32 id;
    GtkWidget *window; /* GtkOffscreenWindow */
    WebKitWebView *view;
    WebKitInputMethodContext *im;

    /* Last requested (clamped) geometry in device pixels, and page scale. */
    int width, height;
    double scale;
    gboolean visible;

    /* Frame production. */
    gboolean dirty;
    gboolean full_damage; /* next frame carries full damage */
    guint8 *prev; /* raw capture of the last handed-over frame, for damage computation */
    int prev_width, prev_height, prev_stride;
    Pool *pool;
    guint64 generation;
    guint64 seq;

    /* Page state. */
    char *error;
    gboolean was_loading;

    /* Menus. */
    WebKitOptionMenu *option_menu;
    gulong option_close_handler;
    char *link; /* link captured by the last context menu */

    /* Last pointer position (logical px) seen in a pointer command. */
    double ptr_x, ptr_y;
} Page;

/* ---- json.c ---- */
JsonObject *vsp_json_parse_object(const char *data, gsize len, JsonParser **parser_out);
const char *vsp_json_string(JsonObject *obj, const char *name, const char *fallback);
double vsp_json_double(JsonObject *obj, const char *name, double fallback);
gint64 vsp_json_int(JsonObject *obj, const char *name, gint64 fallback);
gboolean vsp_json_bool(JsonObject *obj, const char *name, gboolean fallback);
JsonBuilder *vsp_json_begin(void);
char *vsp_json_finish(JsonBuilder *builder, gsize *len); /* consumes builder */

/* ---- channel.c ---- */
gboolean vsp_channel_open(int fd);
void vsp_channel_close(void);
gboolean vsp_channel_alive(void);
/* Allocates a message buffer (header + payload_len) and fills the header; returns the buffer.
 * Payload starts at buffer + VSP_HEADER_LEN. Ownership passes to vsp_channel_submit. */
guint8 *vsp_channel_alloc(char kind, guint32 page, gsize payload_len);
/* Queues the buffer for sending. `fds` (n_fds) are owned by the channel afterwards. */
void vsp_channel_submit(guint8 *msg, const int *fds, int n_fds);
void vsp_send_raw(char kind, guint32 page, const void *payload, gsize len);
void vsp_send_text(char kind, guint32 page, const char *text);
void vsp_send_json_builder(char kind, guint32 page, JsonBuilder *builder); /* consumes */
gsize vsp_channel_pending_bytes(void);
/* Implemented by proto.c: */
void vsp_on_message(char kind, guint32 page, const guint8 *payload, gsize len);
void vsp_on_channel_eof(void);
void vsp_on_protocol_violation(void);

/* ---- policy.c ---- */
gboolean vsp_navigation_allowed(const char *uri);
gboolean vsp_frame_navigation_allowed(const char *uri);

/* ---- page.c ---- */
extern WebKitWebContext *vsp_web_context;
extern gboolean vsp_shutting_down;
void vsp_pages_init(void);
Page *vsp_page_lookup(guint32 id);
Page *vsp_page_create(guint32 id);
void vsp_page_close(guint32 id);
void vsp_pages_close_all(void);
/* Calls fn(key, page, data) for every page; fn must not close pages. */
void vsp_pages_foreach(GHFunc fn, gpointer data);
void vsp_page_send_state(Page *page);
void vsp_page_load(Page *page, const char *url);
void vsp_page_resize(Page *page, int width, int height, double scale);
void vsp_page_set_visible(Page *page, gboolean visible);
void vsp_page_eval(Page *page, const char *script);
void vsp_page_close_option_menu(Page *page);
void vsp_page_option_command(Page *page, const char *cmd);
void vsp_page_link_command(Page *page, const char *cmd);

/* ---- frame.c ---- */
extern TransferMode vsp_transfer_mode;
void vsp_frames_start(void);
void vsp_frame_page_free(Page *page);
void vsp_frame_mark_dirty(Page *page);
void vsp_frame_mark_full(Page *page);
void vsp_frame_set_transfer(TransferMode mode);
void vsp_frame_release(Page *page, guint64 generation, int slot);

/* ---- input.c ---- */
void vsp_input_init(void);
void vsp_input_pointer(Page *page, const char *cmd, JsonObject *obj);
void vsp_input_focus(Page *page, gboolean focused);
void vsp_input_key(Page *page, gboolean press, JsonObject *obj);
void vsp_input_clipboard(Page *page, const char *cmd);
void vsp_input_insert_text(Page *page, const char *text);

/* ---- imctx.c ---- */
WebKitInputMethodContext *vsp_im_new(guint32 page_id);
void vsp_im_preedit(WebKitInputMethodContext *ctx, const char *text);
void vsp_im_commit(WebKitInputMethodContext *ctx, const char *text);
void vsp_im_unmark(WebKitInputMethodContext *ctx);

/* ---- profile.c ---- */
/* Creates the engine context. On failure sends an E message (when `report`), sets *exit_code.
 * Returns NULL on failure. */
WebKitWebContext *vsp_profile_open(int argc, char **argv, int first_arg, int *exit_code);

#endif
