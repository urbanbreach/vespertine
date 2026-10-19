/* The framed message channel (PLT-014 R5, PLT-015 R1/R2): a non-blocking AF_UNIX stream socket.
 * Incoming data is reassembled into messages from a bounded buffer; outgoing messages are queued
 * and written without ever blocking the main loop (file descriptors travel as SCM_RIGHTS). */
#include "vsp.h"

#include <errno.h>
#include <fcntl.h>
#include <glib-unix.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/uio.h>
#include <unistd.h>

#define VSP_MAX_FDS 8
/* A peer that never reads must not make the helper grow without bound. */
#define VSP_MAX_QUEUED_BYTES ((gsize)512 * 1024 * 1024)

typedef struct {
    guint8 *data;
    gsize len;
    gsize off;
    int fds[VSP_MAX_FDS];
    int n_fds;
} OutChunk;

static int channel_fd = -1;
static gboolean channel_dead = FALSE;
static guint read_source = 0;
static guint write_source = 0;
static GQueue out_queue = G_QUEUE_INIT;
static gsize out_bytes = 0;

/* Reassembly buffer: header + at most VSP_MAX_COMMAND_LEN payload. */
static guint8 *rbuf = NULL;
static gsize rlen = 0;

static void put_le32(guint8 *p, guint32 v)
{
    p[0] = v & 0xff;
    p[1] = (v >> 8) & 0xff;
    p[2] = (v >> 16) & 0xff;
    p[3] = (v >> 24) & 0xff;
}

static guint32 get_le32(const guint8 *p)
{
    return (guint32)p[0] | ((guint32)p[1] << 8) | ((guint32)p[2] << 16) | ((guint32)p[3] << 24);
}

static void chunk_free(OutChunk *chunk)
{
    for (int i = 0; i < chunk->n_fds; i++)
        close(chunk->fds[i]);
    g_free(chunk->data);
    g_free(chunk);
}

static void drop_queue(void)
{
    OutChunk *chunk;
    while ((chunk = g_queue_pop_head(&out_queue)) != NULL)
        chunk_free(chunk);
    out_bytes = 0;
    if (write_source) {
        g_source_remove(write_source);
        write_source = 0;
    }
}

static void mark_dead(void)
{
    if (channel_dead)
        return;
    channel_dead = TRUE;
    drop_queue();
    if (read_source) {
        g_source_remove(read_source);
        read_source = 0;
    }
    vsp_on_channel_eof();
}

static gboolean flush_cb(gint fd, GIOCondition cond, gpointer data);

/* Writes as much of the queue as the socket accepts. */
static void flush_queue(void)
{
    OutChunk *chunk;
    while (!channel_dead && (chunk = g_queue_peek_head(&out_queue)) != NULL) {
        struct iovec iov = { .iov_base = chunk->data + chunk->off, .iov_len = chunk->len - chunk->off };
        struct msghdr msg = { 0 };
        union {
            char buf[CMSG_SPACE(sizeof(int) * VSP_MAX_FDS)];
            struct cmsghdr align;
        } control;
        msg.msg_iov = &iov;
        msg.msg_iovlen = 1;
        if (chunk->n_fds > 0) {
            memset(&control, 0, sizeof control);
            msg.msg_control = control.buf;
            msg.msg_controllen = CMSG_SPACE(sizeof(int) * chunk->n_fds);
            struct cmsghdr *cm = CMSG_FIRSTHDR(&msg);
            cm->cmsg_level = SOL_SOCKET;
            cm->cmsg_type = SCM_RIGHTS;
            cm->cmsg_len = CMSG_LEN(sizeof(int) * chunk->n_fds);
            memcpy(CMSG_DATA(cm), chunk->fds, sizeof(int) * chunk->n_fds);
        }
        ssize_t n = sendmsg(channel_fd, &msg, MSG_NOSIGNAL | MSG_DONTWAIT);
        if (n < 0) {
            if (errno == EINTR)
                continue;
            if (errno == EAGAIN || errno == EWOULDBLOCK) {
                if (!write_source)
                    write_source = g_unix_fd_add(channel_fd, G_IO_OUT, flush_cb, NULL);
                return;
            }
            mark_dead();
            return;
        }
        /* Ancillary data is delivered with the first byte of the call that carried it. */
        for (int i = 0; i < chunk->n_fds; i++)
            close(chunk->fds[i]);
        chunk->n_fds = 0;
        chunk->off += (gsize)n;
        out_bytes -= (gsize)n;
        if (chunk->off >= chunk->len) {
            g_queue_pop_head(&out_queue);
            chunk_free(chunk);
        }
    }
    if (write_source && g_queue_is_empty(&out_queue)) {
        g_source_remove(write_source);
        write_source = 0;
    }
}

static gboolean flush_cb(gint fd, GIOCondition cond, gpointer data)
{
    write_source = 0; /* returning REMOVE below; flush_queue may re-add */
    flush_queue();
    return G_SOURCE_REMOVE;
}

guint8 *vsp_channel_alloc(char kind, guint32 page, gsize payload_len)
{
    guint8 *msg = g_malloc(VSP_HEADER_LEN + payload_len);
    msg[0] = (guint8)kind;
    put_le32(msg + 1, page);
    put_le32(msg + 5, (guint32)payload_len);
    return msg;
}

void vsp_channel_submit(guint8 *msg, const int *fds, int n_fds)
{
    gsize total = VSP_HEADER_LEN + get_le32(msg + 5);
    if (channel_dead || channel_fd < 0 || out_bytes + total > VSP_MAX_QUEUED_BYTES) {
        g_free(msg);
        for (int i = 0; i < n_fds; i++)
            close(fds[i]);
        if (!channel_dead && channel_fd >= 0)
            mark_dead();
        return;
    }
    OutChunk *chunk = g_new0(OutChunk, 1);
    chunk->data = msg;
    chunk->len = total;
    g_return_if_fail(n_fds <= VSP_MAX_FDS);
    for (int i = 0; i < n_fds; i++)
        chunk->fds[i] = fds[i];
    chunk->n_fds = n_fds;
    g_queue_push_tail(&out_queue, chunk);
    out_bytes += total;
    if (!write_source)
        flush_queue();
}

void vsp_send_raw(char kind, guint32 page, const void *payload, gsize len)
{
    guint8 *msg = vsp_channel_alloc(kind, page, len);
    if (len)
        memcpy(msg + VSP_HEADER_LEN, payload, len);
    vsp_channel_submit(msg, NULL, 0);
}

void vsp_send_text(char kind, guint32 page, const char *text)
{
    vsp_send_raw(kind, page, text, strlen(text));
}

void vsp_send_json_builder(char kind, guint32 page, JsonBuilder *builder)
{
    gsize len = 0;
    char *text = vsp_json_finish(builder, &len);
    vsp_send_raw(kind, page, text, len);
    g_free(text);
}

gsize vsp_channel_pending_bytes(void)
{
    return out_bytes;
}

gboolean vsp_channel_alive(void)
{
    return !channel_dead && channel_fd >= 0;
}

/* Parses and dispatches complete messages at the front of rbuf. Returns FALSE on a protocol
 * violation. */
static gboolean process_rbuf(void)
{
    gsize pos = 0;
    while (rlen - pos >= VSP_HEADER_LEN) {
        guint32 len = get_le32(rbuf + pos + 5);
        if (len > VSP_MAX_COMMAND_LEN) {
            return FALSE;
        }
        if (rlen - pos < VSP_HEADER_LEN + (gsize)len)
            break;
        char kind = (char)rbuf[pos];
        guint32 page = get_le32(rbuf + pos + 1);
        /* The handler may not touch rbuf; copy nothing, payload stays valid for the call. */
        vsp_on_message(kind, page, rbuf + pos + VSP_HEADER_LEN, len);
        pos += VSP_HEADER_LEN + len;
        if (channel_dead)
            return TRUE;
    }
    if (pos > 0) {
        memmove(rbuf, rbuf + pos, rlen - pos);
        rlen -= pos;
    }
    return TRUE;
}

static gboolean read_cb(gint fd, GIOCondition cond, gpointer data)
{
    for (;;) {
        gsize room = VSP_HEADER_LEN + VSP_MAX_COMMAND_LEN - rlen;
        if (room == 0) {
            /* A full buffer that still does not hold a complete message cannot happen: the
             * length check rejects anything larger than the buffer. */
            break;
        }
        ssize_t n = read(channel_fd, rbuf + rlen, room);
        if (n < 0) {
            if (errno == EINTR)
                continue;
            if (errno == EAGAIN || errno == EWOULDBLOCK)
                return G_SOURCE_CONTINUE;
            read_source = 0;
            mark_dead();
            return G_SOURCE_REMOVE;
        }
        if (n == 0) {
            read_source = 0;
            mark_dead();
            return G_SOURCE_REMOVE;
        }
        rlen += (gsize)n;
        if (!process_rbuf()) {
            read_source = 0;
            vsp_on_protocol_violation();
            return G_SOURCE_REMOVE;
        }
        if (channel_dead)
            return G_SOURCE_REMOVE;
        /* Keep reading while the socket has data, but yield after one buffer's worth so frame
         * ticks and ping answers are not starved by a flood. */
        if ((gsize)n < room)
            return G_SOURCE_CONTINUE;
    }
    return G_SOURCE_CONTINUE;
}

gboolean vsp_channel_open(int fd)
{
    int flags = fcntl(fd, F_GETFD);
    if (flags < 0 || fcntl(fd, F_SETFD, flags | FD_CLOEXEC) < 0)
        return FALSE;
    int fl = fcntl(fd, F_GETFL);
    if (fl < 0 || fcntl(fd, F_SETFL, fl | O_NONBLOCK) < 0)
        return FALSE;
    channel_fd = fd;
    rbuf = g_malloc(VSP_HEADER_LEN + VSP_MAX_COMMAND_LEN);
    read_source = g_unix_fd_add(fd, G_IO_IN | G_IO_HUP | G_IO_ERR, read_cb, NULL);
    return TRUE;
}

void vsp_channel_close(void)
{
    channel_dead = TRUE;
    drop_queue();
    if (read_source) {
        g_source_remove(read_source);
        read_source = 0;
    }
}
