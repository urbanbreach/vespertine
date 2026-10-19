/* Synthetic input (PLT-016 R4, R9, R11), editing commands and clipboard bridging (R15/R16). */
#include "vsp.h"

#include <string.h>

static GdkSeat *seat = NULL;

void vsp_input_init(void)
{
    seat = gdk_display_get_default_seat(gdk_display_get_default());
}

static guint32 now_ms(void)
{
    return (guint32)(g_get_monotonic_time() / 1000);
}

static GdkWindow *view_window(Page *page)
{
    return gtk_widget_get_window(GTK_WIDGET(page->view));
}

static void deliver(Page *page, GdkEvent *event)
{
    gtk_widget_event(GTK_WIDGET(page->view), event);
    gdk_event_free(event);
}

static void set_pointer_device(GdkEvent *event)
{
    GdkDevice *pointer = gdk_seat_get_pointer(seat);
    gdk_event_set_device(event, pointer);
    gdk_event_set_source_device(event, pointer);
}

/* move, down, up, scroll, leave. Coordinates are logical px; the widget works in device px. */
void vsp_input_pointer(Page *page, const char *cmd, JsonObject *obj)
{
    GdkWindow *window = view_window(page);
    if (!window)
        return;
    double lx = vsp_json_double(obj, "x", 0.0);
    double ly = vsp_json_double(obj, "y", 0.0);
    double x = lx * page->scale;
    double y = ly * page->scale;
    guint state = (guint)vsp_json_int(obj, "mods", 0);
    page->ptr_x = lx;
    page->ptr_y = ly;

    GdkEvent *event;
    if (!strcmp(cmd, "move")) {
        event = gdk_event_new(GDK_MOTION_NOTIFY);
        event->motion.window = g_object_ref(window);
        event->motion.send_event = TRUE;
        event->motion.time = now_ms();
        event->motion.x = x;
        event->motion.y = y;
        event->motion.state = state;
        event->motion.is_hint = 0;
        event->motion.x_root = x;
        event->motion.y_root = y;
    } else if (!strcmp(cmd, "down") || !strcmp(cmd, "up")) {
        gboolean press = cmd[0] == 'd';
        guint button = (guint)vsp_json_int(obj, "button", 1);
        if (press)
            gtk_widget_grab_focus(GTK_WIDGET(page->view));
        event = gdk_event_new(press ? GDK_BUTTON_PRESS : GDK_BUTTON_RELEASE);
        event->button.window = g_object_ref(window);
        event->button.send_event = TRUE;
        event->button.time = now_ms();
        event->button.x = x;
        event->button.y = y;
        event->button.state = state;
        event->button.button = button;
        event->button.x_root = x;
        event->button.y_root = y;
    } else if (!strcmp(cmd, "scroll")) {
        event = gdk_event_new(GDK_SCROLL);
        event->scroll.window = g_object_ref(window);
        event->scroll.send_event = TRUE;
        event->scroll.time = now_ms();
        event->scroll.x = x;
        event->scroll.y = y;
        event->scroll.state = state;
        event->scroll.direction = GDK_SCROLL_SMOOTH;
        event->scroll.delta_x = vsp_json_double(obj, "dx", 0.0);
        event->scroll.delta_y = vsp_json_double(obj, "dy", 0.0);
        event->scroll.x_root = x;
        event->scroll.y_root = y;
        event->scroll.is_stop = FALSE;
    } else { /* leave */
        /* The engine treats a crossing event inside the view as an ordinary move, so hover would
         * not end. A real leave carries the position just outside the edge the pointer crossed:
         * project a position inside the view onto the nearest edge, one pixel outside. */
        if (x >= 0 && y >= 0 && x < page->width && y < page->height) {
            double d_left = x, d_right = page->width - 1 - x, d_top = y, d_bottom = page->height - 1 - y;
            double best = MIN(MIN(d_left, d_right), MIN(d_top, d_bottom));
            if (best == d_left)
                x = -1;
            else if (best == d_right)
                x = page->width;
            else if (best == d_top)
                y = -1;
            else
                y = page->height;
        }
        event = gdk_event_new(GDK_LEAVE_NOTIFY);
        event->crossing.window = g_object_ref(window);
        event->crossing.send_event = TRUE;
        event->crossing.subwindow = NULL;
        event->crossing.time = now_ms();
        event->crossing.x = x;
        event->crossing.y = y;
        event->crossing.x_root = x;
        event->crossing.y_root = y;
        event->crossing.mode = GDK_CROSSING_NORMAL;
        event->crossing.detail = GDK_NOTIFY_ANCESTOR;
        event->crossing.focus = FALSE;
        event->crossing.state = state;
    }
    set_pointer_device(event);
    deliver(page, event);
}

void vsp_input_focus(Page *page, gboolean focused)
{
    GdkWindow *window = view_window(page);
    if (!window)
        return;
    if (focused)
        gtk_widget_grab_focus(GTK_WIDGET(page->view));
    GdkEvent *event = gdk_event_new(GDK_FOCUS_CHANGE);
    event->focus_change.window = g_object_ref(window);
    event->focus_change.send_event = TRUE;
    event->focus_change.in = focused ? 1 : 0;
    deliver(page, event);
}

/* Key name resolution of PLT-016 R11. */
void vsp_input_key(Page *page, gboolean press, JsonObject *obj)
{
    GdkWindow *window = view_window(page);
    if (!window)
        return;
    const char *key = vsp_json_string(obj, "key", "");
    const char *text = vsp_json_string(obj, "text", "");
    guint state = (guint)vsp_json_int(obj, "mods", 0);

    guint keyval = *key ? gdk_keyval_from_name(key) : GDK_KEY_VoidSymbol;
    if (keyval == 0)
        keyval = GDK_KEY_VoidSymbol;
    if (keyval == GDK_KEY_VoidSymbol && *key && g_utf8_validate(key, -1, NULL) && g_utf8_strlen(key, -1) == 1) {
        gunichar ch = g_utf8_get_char(key);
        guint candidate = gdk_unicode_to_keyval(ch);
        /* gdk_unicode_to_keyval maps unrepresentable scalars to a 0x01000000 + value keyval. */
        keyval = candidate ? candidate : GDK_KEY_VoidSymbol;
    }

    guint hardware_keycode = 0;
    guint8 group = 0;
    if (keyval != GDK_KEY_VoidSymbol) {
        GdkKeymap *keymap = gdk_keymap_get_for_display(gdk_display_get_default());
        GdkKeymapKey *keys = NULL;
        gint n_keys = 0;
        if (gdk_keymap_get_entries_for_keyval(keymap, keyval, &keys, &n_keys) && n_keys > 0) {
            hardware_keycode = keys[0].keycode;
            group = (guint8)keys[0].group;
        }
        g_free(keys);
    }

    GdkEvent *event = gdk_event_new(press ? GDK_KEY_PRESS : GDK_KEY_RELEASE);
    event->key.window = g_object_ref(window);
    event->key.send_event = TRUE;
    event->key.time = now_ms();
    event->key.state = state;
    event->key.keyval = keyval;
    event->key.length = (gint)strlen(text);
    event->key.string = g_strdup(text);
    event->key.hardware_keycode = (guint16)hardware_keycode;
    event->key.group = group;
    event->key.is_modifier = 0;
    GdkDevice *keyboard = gdk_seat_get_keyboard(seat);
    gdk_event_set_device(event, keyboard);
    gdk_event_set_source_device(event, keyboard);
    deliver(page, event);
}

void vsp_input_insert_text(Page *page, const char *text)
{
    webkit_web_view_execute_editing_command_with_argument(page->view, "InsertText", text);
}

/* The selected text: the slice of a focused text control, else the document selection; empty for
 * password controls (PLT-016 R15). */
static const char *selection_script =
    "(function(){"
    "var a=document.activeElement;"
    "if(a&&(a.tagName==='INPUT'||a.tagName==='TEXTAREA')){"
    "if(a.type==='password')return '';"
    "try{var s=a.selectionStart,e=a.selectionEnd;"
    "if(typeof s==='number'&&typeof e==='number')return a.value.slice(s,e);}catch(x){}"
    "}"
    "var sel=window.getSelection();return sel?sel.toString():'';"
    "})()";

typedef struct {
    guint32 id;
    gboolean cut;
} CopyCtx;

static void on_selection_done(GObject *source, GAsyncResult *result, gpointer data)
{
    CopyCtx *ctx = data;
    GError *error = NULL;
    JSCValue *value = webkit_web_view_evaluate_javascript_finish(WEBKIT_WEB_VIEW(source), result, &error);
    g_clear_error(&error);
    Page *page = vsp_page_lookup(ctx->id);
    if (value && page && !vsp_shutting_down && jsc_value_is_string(value)) {
        char *text = jsc_value_to_string(value);
        if (text && *text) {
            if (ctx->cut)
                webkit_web_view_execute_editing_command(page->view, "Delete");
            vsp_send_text('C', ctx->id, text);
        }
        g_free(text);
    }
    if (value)
        g_object_unref(value);
    g_free(ctx);
}

void vsp_input_clipboard(Page *page, const char *cmd)
{
    if (!strcmp(cmd, "select-all")) {
        webkit_web_view_execute_editing_command(page->view, "SelectAll");
        return;
    }
    CopyCtx *ctx = g_new0(CopyCtx, 1);
    ctx->id = page->id;
    ctx->cut = !strcmp(cmd, "cut");
    webkit_web_view_evaluate_javascript(page->view, selection_script, -1, NULL, NULL, NULL, on_selection_done, ctx);
}
