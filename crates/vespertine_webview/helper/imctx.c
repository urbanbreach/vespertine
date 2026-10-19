/* The custom input-method context of every web view (PLT-014 R7, PLT-016 R12/R13). The framework
 * owns the platform text input: composition arrives as helper commands and the engine's focus,
 * caret and surrounding-text notifications leave as "I" messages. The context never filters key
 * events itself. */
#include "vsp.h"

#include <string.h>

typedef struct {
    WebKitInputMethodContext parent;
    guint32 page_id;
    char *preedit;
    gboolean composing;
} VspIM;

typedef struct {
    WebKitInputMethodContextClass parent_class;
} VspIMClass;

G_DEFINE_TYPE(VspIM, vsp_im, WEBKIT_TYPE_INPUT_METHOD_CONTEXT)

static void send_state(VspIM *im, JsonBuilder *b)
{
    vsp_send_json_builder('I', im->page_id, b);
}

static void im_finalize(GObject *object)
{
    VspIM *im = (VspIM *)object;
    g_free(im->preedit);
    G_OBJECT_CLASS(vsp_im_parent_class)->finalize(object);
}

static void im_set_enable_preedit(WebKitInputMethodContext *context, gboolean enabled) {}

static void im_get_preedit(WebKitInputMethodContext *context, gchar **text, GList **underlines, guint *cursor_offset)
{
    VspIM *im = (VspIM *)context;
    const char *preedit = im->preedit ? im->preedit : "";
    guint chars = (guint)g_utf8_strlen(preedit, -1);
    if (text)
        *text = g_strdup(preedit);
    if (underlines) {
        *underlines = NULL;
        if (chars > 0)
            *underlines = g_list_append(NULL, webkit_input_method_underline_new(0, chars));
    }
    if (cursor_offset)
        *cursor_offset = chars;
}

static gboolean im_filter_key_event(WebKitInputMethodContext *context, GdkEventKey *key_event)
{
    return FALSE;
}

static void im_notify_focus_in(WebKitInputMethodContext *context)
{
    VspIM *im = (VspIM *)context;
    JsonBuilder *b = vsp_json_begin();
    json_builder_set_member_name(b, "focused");
    json_builder_add_boolean_value(b, TRUE);
    send_state(im, b);
}

static void im_notify_focus_out(WebKitInputMethodContext *context)
{
    VspIM *im = (VspIM *)context;
    JsonBuilder *b = vsp_json_begin();
    json_builder_set_member_name(b, "focused");
    json_builder_add_boolean_value(b, FALSE);
    send_state(im, b);
}

static void im_notify_cursor_area(WebKitInputMethodContext *context, int x, int y, int width, int height)
{
    VspIM *im = (VspIM *)context;
    JsonBuilder *b = vsp_json_begin();
    json_builder_set_member_name(b, "caret");
    json_builder_begin_array(b);
    json_builder_add_int_value(b, x);
    json_builder_add_int_value(b, y);
    json_builder_add_int_value(b, width);
    json_builder_add_int_value(b, height);
    json_builder_end_array(b);
    send_state(im, b);
}

static void im_notify_surrounding(WebKitInputMethodContext *context, const gchar *text, guint length, guint cursor_index, guint selection_index)
{
    VspIM *im = (VspIM *)context;
    JsonBuilder *b = vsp_json_begin();
    json_builder_set_member_name(b, "focused");
    json_builder_add_boolean_value(b, TRUE);
    /* Password contents never leave the engine: an empty text replaces any earlier state. */
    gboolean password = webkit_input_method_context_get_input_purpose(context) == WEBKIT_INPUT_PURPOSE_PASSWORD;
    json_builder_set_member_name(b, "text");
    if (password || !text) {
        json_builder_add_string_value(b, "");
        json_builder_set_member_name(b, "cursor");
        json_builder_add_int_value(b, 0);
        json_builder_set_member_name(b, "selection");
        json_builder_add_int_value(b, 0);
    } else {
        /* The engine may pass a length that is shorter than the NUL-terminated string. */
        gsize len = (gint)length < 0 ? strlen(text) : MIN((gsize)length, strlen(text));
        char *copy = g_utf8_make_valid(text, (gssize)len);
        json_builder_add_string_value(b, copy);
        g_free(copy);
        json_builder_set_member_name(b, "cursor");
        json_builder_add_int_value(b, cursor_index);
        json_builder_set_member_name(b, "selection");
        json_builder_add_int_value(b, selection_index);
    }
    send_state(im, b);
}

static void im_reset(WebKitInputMethodContext *context) {}

static void vsp_im_class_init(VspIMClass *klass)
{
    G_OBJECT_CLASS(klass)->finalize = im_finalize;
    WebKitInputMethodContextClass *base = WEBKIT_INPUT_METHOD_CONTEXT_CLASS(klass);
    base->set_enable_preedit = im_set_enable_preedit;
    base->get_preedit = im_get_preedit;
    base->filter_key_event = im_filter_key_event;
    base->notify_focus_in = im_notify_focus_in;
    base->notify_focus_out = im_notify_focus_out;
    base->notify_cursor_area = im_notify_cursor_area;
    base->notify_surrounding = im_notify_surrounding;
    base->reset = im_reset;
}

static void vsp_im_init(VspIM *im) {}

WebKitInputMethodContext *vsp_im_new(guint32 page_id)
{
    VspIM *im = g_object_new(vsp_im_get_type(), NULL);
    im->page_id = page_id;
    return WEBKIT_INPUT_METHOD_CONTEXT(im);
}

/* "preedit": replaces the composition string; started once per composition. */
void vsp_im_preedit(WebKitInputMethodContext *ctx, const char *text)
{
    VspIM *im = (VspIM *)ctx;
    if (!im->composing) {
        im->composing = TRUE;
        g_signal_emit_by_name(ctx, "preedit-started");
    }
    g_free(im->preedit);
    im->preedit = g_strdup(text);
    g_signal_emit_by_name(ctx, "preedit-changed");
}

static void finish(VspIM *im, const char *commit)
{
    if (commit)
        g_signal_emit_by_name(im, "committed", commit);
    g_clear_pointer(&im->preedit, g_free);
    g_signal_emit_by_name(im, "preedit-changed");
    g_signal_emit_by_name(im, "preedit-finished");
    im->composing = FALSE;
}

void vsp_im_commit(WebKitInputMethodContext *ctx, const char *text)
{
    finish((VspIM *)ctx, text);
}

void vsp_im_unmark(WebKitInputMethodContext *ctx)
{
    finish((VspIM *)ctx, NULL);
}
