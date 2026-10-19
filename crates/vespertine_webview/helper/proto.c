/* Command dispatch (PLT-015 R4) and the shutdown sequence (PLT-014 R12). */
#include "vsp.h"

#include <string.h>

extern GMainLoop *vsp_main_loop;

static void on_cookies_flushed(GObject *source, GAsyncResult *result, gpointer data)
{
    GError *error = NULL;
    GList *cookies = webkit_cookie_manager_get_cookies_finish(WEBKIT_COOKIE_MANAGER(source), result, &error);
    g_list_free_full(cookies, (GDestroyNotify)soup_cookie_free);
    g_clear_error(&error);
    g_main_loop_quit(vsp_main_loop);
}

/* Ignore all further commands, destroy every page (closing option menus first), then ask the
 * cookie manager for the cookies of a placeholder address so the engine finishes pending cookie
 * writes; the main loop ends when that request completes. */
static void begin_shutdown(void)
{
    if (vsp_shutting_down)
        return;
    vsp_shutting_down = TRUE;
    vsp_pages_close_all();
    WebKitWebsiteDataManager *manager = webkit_web_context_get_website_data_manager(vsp_web_context);
    WebKitCookieManager *cookies = webkit_website_data_manager_get_cookie_manager(manager);
    webkit_cookie_manager_get_cookies(cookies, "http://localhost/", NULL, on_cookies_flushed, NULL);
}

static gboolean begin_shutdown_idle(gpointer data)
{
    begin_shutdown();
    return G_SOURCE_REMOVE;
}

/* May be reached from inside a page iteration (a failed write in the frame tick): shut down from
 * the main loop instead of destroying pages underneath the iteration. */
void vsp_on_channel_eof(void)
{
    g_idle_add(begin_shutdown_idle, NULL);
}

void vsp_on_protocol_violation(void)
{
    /* A message larger than 1 MiB: stop reading and leave the loop immediately. */
    vsp_shutting_down = TRUE;
    g_main_loop_quit(vsp_main_loop);
}

static void handle_configure(JsonObject *obj)
{
    const char *transfer = vsp_json_string(obj, "transfer", "");
    if (!strcmp(transfer, "inline"))
        vsp_frame_set_transfer(TRANSFER_INLINE);
    else if (!strcmp(transfer, "shm"))
        vsp_frame_set_transfer(TRANSFER_SHM);
}

static void handle_ping(JsonObject *obj)
{
    JsonBuilder *b = vsp_json_begin();
    json_builder_set_member_name(b, "nonce");
    json_builder_add_int_value(b, vsp_json_int(obj, "nonce", 0));
    vsp_send_json_builder('Z', 0, b);
}

static void handle_page_command(Page *page, const char *cmd, JsonObject *obj)
{
    if (!strcmp(cmd, "close")) {
        vsp_page_close(page->id);
    } else if (!strcmp(cmd, "release")) {
        vsp_frame_release(page, (guint64)vsp_json_int(obj, "generation", 0), (int)CLAMP(vsp_json_int(obj, "slot", -1), -1, 1 << 20));
    } else if (!strcmp(cmd, "load")) {
        vsp_page_load(page, vsp_json_string(obj, "url", ""));
    } else if (!strcmp(cmd, "reload")) {
        webkit_web_view_reload(page->view);
    } else if (!strcmp(cmd, "back")) {
        webkit_web_view_go_back(page->view);
    } else if (!strcmp(cmd, "forward")) {
        webkit_web_view_go_forward(page->view);
    } else if (!strcmp(cmd, "resize")) {
        vsp_page_resize(page, (int)CLAMP(vsp_json_int(obj, "width", 0), -1, 1 << 20), (int)CLAMP(vsp_json_int(obj, "height", 0), -1, 1 << 20), vsp_json_double(obj, "scale", 0.0));
    } else if (!strcmp(cmd, "visible")) {
        vsp_page_set_visible(page, vsp_json_bool(obj, "value", FALSE));
    } else if (!strcmp(cmd, "dismiss-menu")) {
        vsp_page_close_option_menu(page);
    } else if (!strncmp(cmd, "option:", 7)) {
        vsp_page_option_command(page, cmd);
    } else if (!strcmp(cmd, "open-link") || !strcmp(cmd, "copy-link")) {
        vsp_page_link_command(page, cmd);
    } else if (!strcmp(cmd, "move") || !strcmp(cmd, "down") || !strcmp(cmd, "up") || !strcmp(cmd, "scroll") || !strcmp(cmd, "leave")) {
        vsp_input_pointer(page, cmd, obj);
    } else if (!strcmp(cmd, "focus")) {
        vsp_input_focus(page, vsp_json_bool(obj, "value", FALSE));
    } else if (!strcmp(cmd, "key_down") || !strcmp(cmd, "key_up")) {
        vsp_input_key(page, cmd[4] == 'd', obj);
    } else if (!strcmp(cmd, "preedit")) {
        vsp_im_preedit(page->im, vsp_json_string(obj, "text", ""));
    } else if (!strcmp(cmd, "commit")) {
        vsp_im_commit(page->im, vsp_json_string(obj, "text", ""));
    } else if (!strcmp(cmd, "unmark")) {
        vsp_im_unmark(page->im);
    } else if (!strcmp(cmd, "text")) {
        vsp_input_insert_text(page, vsp_json_string(obj, "text", ""));
    } else if (!strcmp(cmd, "copy") || !strcmp(cmd, "cut") || !strcmp(cmd, "select-all")) {
        vsp_input_clipboard(page, cmd);
    } else if (!strcmp(cmd, "eval")) {
        vsp_page_eval(page, vsp_json_string(obj, "script", ""));
    }
    /* Anything else is an unknown command and is ignored. */
}

void vsp_on_message(char kind, guint32 page_id, const guint8 *payload, gsize len)
{
    if (kind != 'K')
        return; /* unknown kinds are skipped */
    JsonParser *parser = NULL;
    JsonObject *obj = vsp_json_parse_object((const char *)payload, len, &parser);
    if (!obj)
        return; /* not an object: no effect */
    const char *cmd = vsp_json_string(obj, "cmd", "");

    if (vsp_shutting_down) {
        /* Everything but shutdown is ignored; shutdown while shutting down is a no-op. */
        goto done;
    }
    if (page_id == 0) {
        if (!strcmp(cmd, "configure"))
            handle_configure(obj);
        else if (!strcmp(cmd, "ping"))
            handle_ping(obj);
        else if (!strcmp(cmd, "shutdown"))
            begin_shutdown();
        goto done;
    }
    if (!strcmp(cmd, "create")) {
        vsp_page_create(page_id); /* ignored when the page exists */
        goto done;
    }
    {
        Page *page = vsp_page_lookup(page_id);
        if (page)
            handle_page_command(page, cmd, obj);
    }
done:
    g_object_unref(parser);
}
