/* Frame production and pixel transfer (PLT-015 R7 to R11). A single 16 ms timer visits every
 * page; a visible dirty page produces at most one frame per tick. */
#include "vsp.h"

#include <errno.h>
#include <fcntl.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

typedef struct {
    int fd; /* only valid between creation and announcement */
    guint8 *map;
    gsize size;
    gboolean owned; /* handed to the application, not yet released */
} Slot;

struct Pool {
    guint64 generation;
    int width, height;
    double scale;
    gsize stride;
    gsize size;
    Slot slots[VSP_POOL_SLOTS];
};

TransferMode vsp_transfer_mode = TRANSFER_INLINE;
static gboolean pool_failure_logged = FALSE;

static void put_le32(guint8 *p, guint32 v)
{
    p[0] = v & 0xff;
    p[1] = (v >> 8) & 0xff;
    p[2] = (v >> 16) & 0xff;
    p[3] = (v >> 24) & 0xff;
}

/* ---- damage ---- */

/* A page is dirty when its offscreen window reports damage, after resize and when it becomes
 * visible. Damage events only say "something was drawn": the engine redraws for animation ticks
 * without changing pixels. The damage rectangle of a frame is therefore computed by comparing the
 * capture with the last frame that was handed over, so unchanged pages produce nothing and
 * every frame carries the exact bounding box of the pixels that changed since the last
 * handed-over frame (also while buffers are withheld). */
void vsp_frame_mark_full(Page *page)
{
    page->dirty = TRUE;
    page->full_damage = TRUE;
}

void vsp_frame_mark_dirty(Page *page)
{
    page->dirty = TRUE;
}

/* Bounding box of the pixels that differ between two captures of the same size. */
static gboolean diff_bbox(const guint8 *a, const guint8 *b, int stride, int width, int height, int *x0, int *y0, int *x1, int *y1)
{
    int min_x = width, max_x = -1, min_y = -1, max_y = -1;
    for (int y = 0; y < height; y++) {
        const guint8 *ra = a + (gsize)y * (gsize)stride;
        const guint8 *rb = b + (gsize)y * (gsize)stride;
        if (memcmp(ra, rb, (gsize)width * 4) == 0)
            continue;
        if (min_y < 0)
            min_y = y;
        max_y = y;
        const guint32 *pa = (const guint32 *)ra;
        const guint32 *pb = (const guint32 *)rb;
        for (int x = 0; x < min_x; x++) {
            if (pa[x] != pb[x]) {
                min_x = x;
                break;
            }
        }
        for (int x = width - 1; x > max_x; x--) {
            if (pa[x] != pb[x]) {
                max_x = x;
                break;
            }
        }
    }
    if (max_y < 0)
        return FALSE;
    *x0 = min_x;
    *y0 = min_y;
    *x1 = max_x + 1;
    *y1 = max_y + 1;
    return TRUE;
}

/* ---- pool ---- */

static void pool_free(Pool *pool)
{
    if (!pool)
        return;
    for (int i = 0; i < VSP_POOL_SLOTS; i++) {
        if (pool->slots[i].map && pool->slots[i].map != MAP_FAILED)
            munmap(pool->slots[i].map, pool->slots[i].size);
        if (pool->slots[i].fd >= 0)
            close(pool->slots[i].fd);
    }
    g_free(pool);
}

void vsp_frame_page_free(Page *page)
{
    pool_free(page->pool);
    page->pool = NULL;
    g_clear_pointer(&page->prev, g_free);
}

static Pool *pool_new(guint64 generation, int width, int height, double scale)
{
    Pool *pool = g_new0(Pool, 1);
    pool->generation = generation;
    pool->width = width;
    pool->height = height;
    pool->scale = scale;
    pool->stride = ((gsize)width * 4 + VSP_STRIDE_ALIGN - 1) / VSP_STRIDE_ALIGN * VSP_STRIDE_ALIGN;
    pool->size = pool->stride * (gsize)height;
    for (int i = 0; i < VSP_POOL_SLOTS; i++) {
        Slot *slot = &pool->slots[i];
        slot->fd = -1;
        slot->size = pool->size;
    }
    for (int i = 0; i < VSP_POOL_SLOTS; i++) {
        Slot *slot = &pool->slots[i];
        slot->fd = memfd_create("vsp-frame", MFD_CLOEXEC | MFD_ALLOW_SEALING);
        if (slot->fd < 0 || ftruncate(slot->fd, (off_t)pool->size) != 0)
            goto fail;
        slot->map = mmap(NULL, pool->size, PROT_READ | PROT_WRITE, MAP_SHARED, slot->fd, 0);
        if (slot->map == MAP_FAILED) {
            slot->map = NULL;
            goto fail;
        }
        if (fcntl(slot->fd, F_ADD_SEALS, F_SEAL_SHRINK | F_SEAL_GROW) != 0)
            goto fail;
    }
    return pool;
fail:
    if (!pool_failure_logged) {
        pool_failure_logged = TRUE;
        g_printerr("vsp-webkit-helper: shared-memory pool allocation failed (%s); using inline frames\n", g_strerror(errno));
    }
    pool_free(pool);
    return NULL;
}

/* Announces the pool (kind P) with one descriptor per slot. Ownership of the descriptors moves to
 * the channel; the helper keeps its mappings. */
static void pool_announce(Page *page, Pool *pool)
{
    JsonBuilder *b = vsp_json_begin();
    json_builder_set_member_name(b, "generation");
    json_builder_add_int_value(b, (gint64)pool->generation);
    json_builder_set_member_name(b, "kind");
    json_builder_add_string_value(b, "shm");
    json_builder_set_member_name(b, "width");
    json_builder_add_int_value(b, pool->width);
    json_builder_set_member_name(b, "height");
    json_builder_add_int_value(b, pool->height);
    json_builder_set_member_name(b, "scale");
    json_builder_add_double_value(b, pool->scale);
    json_builder_set_member_name(b, "stride");
    json_builder_add_int_value(b, (gint64)pool->stride);
    json_builder_set_member_name(b, "format");
    json_builder_add_string_value(b, "BGRA8888");
    json_builder_set_member_name(b, "slots");
    json_builder_begin_array(b);
    int fds[VSP_POOL_SLOTS];
    for (int i = 0; i < VSP_POOL_SLOTS; i++) {
        json_builder_begin_object(b);
        json_builder_set_member_name(b, "slot");
        json_builder_add_int_value(b, i);
        json_builder_set_member_name(b, "size");
        json_builder_add_int_value(b, (gint64)pool->slots[i].size);
        json_builder_end_object(b);
        fds[i] = pool->slots[i].fd;
        pool->slots[i].fd = -1;
    }
    json_builder_end_array(b);
    gsize len = 0;
    char *text = vsp_json_finish(b, &len);
    guint8 *msg = vsp_channel_alloc('P', page->id, len);
    memcpy(msg + VSP_HEADER_LEN, text, len);
    g_free(text);
    vsp_channel_submit(msg, fds, VSP_POOL_SLOTS);
}

static void reset_page_transfer(gpointer key, gpointer value, gpointer data)
{
    Page *page = value;
    /* A mode change invalidates the pool; the next frame is a full frame in the new mode. */
    vsp_frame_page_free(page);
    vsp_frame_mark_full(page);
}

void vsp_frame_set_transfer(TransferMode mode)
{
    if (mode == vsp_transfer_mode)
        return;
    vsp_transfer_mode = mode;
    vsp_pages_foreach(reset_page_transfer, NULL);
}

/* "release": the application returns a slot. Unknown generation or slot is ignored. */
void vsp_frame_release(Page *page, guint64 generation, int slot)
{
    Pool *pool = page->pool;
    if (!pool || pool->generation != generation || slot < 0 || slot >= VSP_POOL_SLOTS)
        return;
    pool->slots[slot].owned = FALSE;
}

/* ---- conversion ---- */

/* Cairo ARGB32 (premultiplied, native-endian; bytes B,G,R,A on little-endian hosts) to straight
 * B,G,R,A. Pixels with alpha 255 pass unchanged. */
static void convert_row(const guint8 *src, guint8 *dst, int width, gboolean has_alpha)
{
    for (int x = 0; x < width; x++, src += 4, dst += 4) {
        guint32 a = has_alpha ? src[3] : 255;
        if (a == 255) {
            dst[0] = src[0];
            dst[1] = src[1];
            dst[2] = src[2];
            dst[3] = 255;
        } else if (a == 0) {
            dst[0] = dst[1] = dst[2] = dst[3] = 0;
        } else {
            guint32 half = a / 2;
            guint32 c0 = (src[0] * 255u + half) / a;
            guint32 c1 = (src[1] * 255u + half) / a;
            guint32 c2 = (src[2] * 255u + half) / a;
            dst[0] = (guint8)MIN(c0, 255u);
            dst[1] = (guint8)MIN(c1, 255u);
            dst[2] = (guint8)MIN(c2, 255u);
            dst[3] = (guint8)a;
        }
    }
}

/* ---- production ---- */

static void produce_inline(Page *page, const guint8 *data, int stride, int width, int height, gboolean has_alpha)
{
    gsize pixels = (gsize)width * (gsize)height * 4;
    guint8 *msg = vsp_channel_alloc('F', page->id, 12 + pixels);
    guint8 *p = msg + VSP_HEADER_LEN;
    put_le32(p, (guint32)width);
    put_le32(p + 4, (guint32)height);
    float scale = (float)page->scale;
    guint32 bits;
    memcpy(&bits, &scale, sizeof bits);
    put_le32(p + 8, bits);
    guint8 *dst = p + 12;
    for (int y = 0; y < height; y++)
        convert_row(data + (gsize)y * (gsize)stride, dst + (gsize)y * (gsize)width * 4, width, has_alpha);
    vsp_channel_submit(msg, NULL, 0);
}

/* Returns TRUE when a buffer frame was handed over (or an inline fallback was sent). */
static gboolean produce_shm(Page *page, const guint8 *data, int stride, int width, int height, gboolean has_alpha, const int *damage)
{
    Pool *pool = page->pool;
    gboolean fresh = FALSE;
    if (!pool || pool->width != width || pool->height != height || pool->scale != page->scale) {
        Pool *next = pool_new(page->generation + 1, width, height, page->scale);
        if (!next) {
            produce_inline(page, data, stride, width, height, has_alpha);
            return TRUE;
        }
        pool_free(pool);
        pool = page->pool = next;
        page->generation = pool->generation;
        pool_announce(page, pool);
        fresh = TRUE;
    }

    int slot = -1;
    for (int i = 0; i < VSP_POOL_SLOTS; i++) {
        if (!pool->slots[i].owned) {
            slot = i;
            break;
        }
    }
    if (slot < 0)
        return FALSE; /* both buffers are with the application: keep dirty, accumulate damage */

    guint8 *dst = pool->slots[slot].map;
    for (int y = 0; y < height; y++)
        convert_row(data + (gsize)y * (gsize)stride, dst + (gsize)y * pool->stride, width, has_alpha);

    /* The first frame of a generation carries the full frame. */
    int dx = fresh ? 0 : damage[0], dy = fresh ? 0 : damage[1];
    int dw = fresh ? width : damage[2], dh = fresh ? height : damage[3];

    pool->slots[slot].owned = TRUE;
    page->seq++;
    JsonBuilder *b = vsp_json_begin();
    json_builder_set_member_name(b, "generation");
    json_builder_add_int_value(b, (gint64)pool->generation);
    json_builder_set_member_name(b, "slot");
    json_builder_add_int_value(b, slot);
    json_builder_set_member_name(b, "seq");
    json_builder_add_int_value(b, (gint64)page->seq);
    json_builder_set_member_name(b, "width");
    json_builder_add_int_value(b, width);
    json_builder_set_member_name(b, "height");
    json_builder_add_int_value(b, height);
    json_builder_set_member_name(b, "scale");
    json_builder_add_double_value(b, page->scale);
    json_builder_set_member_name(b, "damage");
    json_builder_begin_array(b);
    json_builder_add_int_value(b, dx);
    json_builder_add_int_value(b, dy);
    json_builder_add_int_value(b, dw);
    json_builder_add_int_value(b, dh);
    json_builder_end_array(b);
    vsp_send_json_builder('B', page->id, b);
    return TRUE;
}

/* Captures the offscreen window as an image surface. On Wayland GTK renders offscreen windows
 * into an image surface; on X11 the surface is a server-side pixmap that is copied into one. The
 * returned surface is owned by the caller (reference) and has the size of the window. */
static cairo_surface_t *capture_surface(Page *page)
{
    cairo_surface_t *surface = gtk_offscreen_window_get_surface(GTK_OFFSCREEN_WINDOW(page->window));
    if (!surface)
        return NULL;
    if (cairo_surface_get_type(surface) == CAIRO_SURFACE_TYPE_IMAGE)
        return cairo_surface_reference(surface);
    int width = gtk_widget_get_allocated_width(page->window);
    int height = gtk_widget_get_allocated_height(page->window);
    if (width != page->width || height != page->height)
        return NULL;
    cairo_surface_t *copy = cairo_image_surface_create(CAIRO_FORMAT_ARGB32, width, height);
    cairo_t *cr = cairo_create(copy);
    cairo_set_operator(cr, CAIRO_OPERATOR_SOURCE);
    cairo_set_source_surface(cr, surface, 0, 0);
    cairo_paint(cr);
    cairo_destroy(cr);
    if (cairo_surface_status(copy) != CAIRO_STATUS_SUCCESS) {
        cairo_surface_destroy(copy);
        return NULL;
    }
    return copy;
}

static void produce_frame(Page *page)
{
    cairo_surface_t *surface = capture_surface(page);
    if (!surface)
        return;
    cairo_format_t format = cairo_image_surface_get_format(surface);
    int width = cairo_image_surface_get_width(surface);
    int height = cairo_image_surface_get_height(surface);
    /* The captured size must equal the last requested size; otherwise the engine has not laid out
     * yet: drop the frame and stay dirty. */
    if ((format != CAIRO_FORMAT_ARGB32 && format != CAIRO_FORMAT_RGB24) || width != page->width ||
        height != page->height || width > VSP_MAX_DIM || height > VSP_MAX_DIM) {
        cairo_surface_destroy(surface);
        return;
    }
    cairo_surface_flush(surface);
    const guint8 *data = cairo_image_surface_get_data(surface);
    int stride = cairo_image_surface_get_stride(surface);
    gboolean has_alpha = format == CAIRO_FORMAT_ARGB32;

    int damage[4] = { 0, 0, width, height };
    gboolean have_prev = page->prev && page->prev_width == width && page->prev_height == height && page->prev_stride == stride;
    if (data && have_prev && !page->full_damage) {
        int x0, y0, x1, y1;
        if (!diff_bbox(data, page->prev, stride, width, height, &x0, &y0, &x1, &y1)) {
            /* The engine drew without changing a pixel: nothing to hand over. */
            cairo_surface_destroy(surface);
            page->dirty = FALSE;
            return;
        }
        damage[0] = x0;
        damage[1] = y0;
        damage[2] = x1 - x0;
        damage[3] = y1 - y0;
    }

    gboolean sent = FALSE;
    if (!data) {
        /* nothing to read */
    } else if (vsp_transfer_mode == TRANSFER_SHM) {
        sent = produce_shm(page, data, stride, width, height, has_alpha, damage);
    } else if (vsp_channel_pending_bytes() == 0) {
        /* Latest-wins backpressure: never queue a frame behind an unsent one. */
        produce_inline(page, data, stride, width, height, has_alpha);
        sent = TRUE;
    }
    if (sent) {
        gsize size = (gsize)stride * (gsize)height;
        if (!page->prev || page->prev_width != width || page->prev_height != height || page->prev_stride != stride)
            page->prev = g_realloc(page->prev, size);
        memcpy(page->prev, data, size);
        page->prev_width = width;
        page->prev_height = height;
        page->prev_stride = stride;
        page->dirty = FALSE;
        page->full_damage = FALSE;
    }
    cairo_surface_destroy(surface);
}

static void tick_page(gpointer key, gpointer value, gpointer data)
{
    Page *page = value;
    if (page->visible && page->dirty)
        produce_frame(page);
}

static gboolean on_tick(gpointer data)
{
    if (vsp_shutting_down || !vsp_channel_alive())
        return G_SOURCE_CONTINUE;
    vsp_pages_foreach(tick_page, NULL);
    return G_SOURCE_CONTINUE;
}

void vsp_frames_start(void)
{
    g_timeout_add(VSP_FRAME_TICK_MS, on_tick, NULL);
}
