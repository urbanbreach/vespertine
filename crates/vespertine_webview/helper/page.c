/* Pages: one WebKitWebView inside one GtkOffscreenWindow (PLT-014 R1/R7/R10, PLT-015 R4/R5,
 * PLT-016 R13/CMP-022 R14 menus). */
#include "vsp.h"

#include <math.h>
#include <string.h>

WebKitWebContext *vsp_web_context = NULL;
gboolean vsp_shutting_down = FALSE;

static GHashTable *pages = NULL;
static WebKitSettings *settings = NULL;
static WebKitUserContentManager *content_manager = NULL;

static void page_free(gpointer data)
{
    Page *page = data;
    g_free(page->error);
    g_free(page->link);
    g_free(page);
}

Page *vsp_page_lookup(guint32 id)
{
    return id ? g_hash_table_lookup(pages, GUINT_TO_POINTER(id)) : NULL;
}

/* ---------------------------------------------------------------------------------------------
 * State messages (PLT-015 R5 "S")
 * ------------------------------------------------------------------------------------------ */

void vsp_page_send_state(Page *page)
{
    if (vsp_shutting_down)
        return;
    JsonBuilder *b = vsp_json_begin();
    const char *uri = webkit_web_view_get_uri(page->view);
    const char *title = webkit_web_view_get_title(page->view);
    json_builder_set_member_name(b, "url");
    if (uri)
        json_builder_add_string_value(b, uri);
    else
        json_builder_add_null_value(b);
    json_builder_set_member_name(b, "title");
    json_builder_add_string_value(b, title ? title : "");
    json_builder_set_member_name(b, "error");
    if (page->error)
        json_builder_add_string_value(b, page->error);
    else
        json_builder_add_null_value(b);
    json_builder_set_member_name(b, "loading");
    json_builder_add_boolean_value(b, webkit_web_view_is_loading(page->view));
    json_builder_set_member_name(b, "can_back");
    json_builder_add_boolean_value(b, webkit_web_view_can_go_back(page->view));
    json_builder_set_member_name(b, "can_forward");
    json_builder_add_boolean_value(b, webkit_web_view_can_go_forward(page->view));
    vsp_send_json_builder('S', page->id, b);
}

static void on_state_notify(GObject *object, GParamSpec *pspec, Page *page)
{
    vsp_page_send_state(page);
}

/* A new load start clears the error string before any snapshot of the new load is sent. */
static void on_loading_notify(GObject *object, GParamSpec *pspec, Page *page)
{
    gboolean loading = webkit_web_view_is_loading(page->view);
    if (loading && !page->was_loading)
        g_clear_pointer(&page->error, g_free);
    page->was_loading = loading;
    vsp_page_send_state(page);
}

static void on_load_changed(WebKitWebView *view, WebKitLoadEvent event, Page *page)
{
    if (event == WEBKIT_LOAD_STARTED)
        g_clear_pointer(&page->error, g_free);
    vsp_page_send_state(page);
}

static gboolean on_load_failed(WebKitWebView *view, WebKitLoadEvent event, const char *uri, GError *error, Page *page)
{
    if (g_error_matches(error, WEBKIT_NETWORK_ERROR, WEBKIT_NETWORK_ERROR_CANCELLED))
        return FALSE;
    g_free(page->error);
    page->error = g_strdup(error && error->message ? error->message : "The page could not be loaded.");
    vsp_page_send_state(page);
    return FALSE;
}

static void on_web_process_terminated(WebKitWebView *view, WebKitWebProcessTerminationReason reason, Page *page)
{
    g_free(page->error);
    page->error = g_strdup("The browser process stopped. Reload to try again.");
    vsp_page_send_state(page);
}

/* ---------------------------------------------------------------------------------------------
 * Policy (PLT-014 R7/R10)
 * ------------------------------------------------------------------------------------------ */

static gboolean on_decide_policy(WebKitWebView *view, WebKitPolicyDecision *decision, WebKitPolicyDecisionType type, Page *page)
{
    if (type == WEBKIT_POLICY_DECISION_TYPE_NAVIGATION_ACTION) {
        WebKitNavigationPolicyDecision *nav = WEBKIT_NAVIGATION_POLICY_DECISION(decision);
        WebKitNavigationAction *action = webkit_navigation_policy_decision_get_navigation_action(nav);
        WebKitURIRequest *request = webkit_navigation_action_get_request(action);
        const char *uri = request ? webkit_uri_request_get_uri(request) : NULL;
        /* PLT-014 R10: the engine exposes only the target frame's name (absent for the main
         * frame), so a navigation with a frame name is a sub-frame navigation. WebKitGTK 2.54
         * reports no name for most sub-frame navigations either (iframe src, data: and about:
         * documents), which would make every iframe load fail; a nameless navigation to a
         * document of the page itself (about:, data:, blob:) is therefore allowed while a web
         * page is shown. The engine itself refuses top-level navigations to such documents. */
        const char *frame = webkit_navigation_action_get_frame_name(action);
        gboolean allowed;
        if (frame)
            allowed = vsp_frame_navigation_allowed(uri);
        else if (vsp_navigation_allowed(uri))
            allowed = TRUE;
        else
            allowed = vsp_frame_navigation_allowed(uri) && vsp_navigation_allowed(webkit_web_view_get_uri(view));
        if (allowed)
            webkit_policy_decision_use(decision);
        else
            webkit_policy_decision_ignore(decision);
        return TRUE;
    }
    if (type == WEBKIT_POLICY_DECISION_TYPE_NEW_WINDOW_ACTION) {
        WebKitNavigationPolicyDecision *nav = WEBKIT_NAVIGATION_POLICY_DECISION(decision);
        WebKitNavigationAction *action = webkit_navigation_policy_decision_get_navigation_action(nav);
        WebKitURIRequest *request = webkit_navigation_action_get_request(action);
        const char *uri = request ? webkit_uri_request_get_uri(request) : NULL;
        if (webkit_navigation_action_is_user_gesture(action) && vsp_navigation_allowed(uri))
            vsp_send_text('N', page->id, uri);
        webkit_policy_decision_ignore(decision);
        return TRUE;
    }
    return FALSE; /* responses: engine default (unsupported content becomes a cancelled download) */
}

/* window.open() reaches the helper as the "create" signal (not as a policy decision). A web view
 * is never created; a user-initiated request for an allowed address becomes a new-tab message. */
static GtkWidget *on_create(WebKitWebView *view, WebKitNavigationAction *action, Page *page)
{
    WebKitURIRequest *request = webkit_navigation_action_get_request(action);
    const char *uri = request ? webkit_uri_request_get_uri(request) : NULL;
    if (webkit_navigation_action_is_user_gesture(action) && vsp_navigation_allowed(uri))
        vsp_send_text('N', page->id, uri);
    return NULL;
}

static gboolean on_permission_request(WebKitWebView *view, WebKitPermissionRequest *request, Page *page)
{
    webkit_permission_request_deny(request);
    return TRUE;
}

static gboolean on_query_permission_state(WebKitWebView *view, WebKitPermissionStateQuery *query, Page *page)
{
    webkit_permission_state_query_finish(query, WEBKIT_PERMISSION_STATE_DENIED);
    return TRUE;
}

static void on_download_started(WebKitWebContext *context, WebKitDownload *download, gpointer data)
{
    webkit_download_cancel(download);
}

/* Dialogs the helper cannot show must never block the page on an invisible window. */
static gboolean on_script_dialog(WebKitWebView *view, WebKitScriptDialog *dialog, Page *page)
{
    switch (webkit_script_dialog_get_dialog_type(dialog)) {
    case WEBKIT_SCRIPT_DIALOG_CONFIRM:
        webkit_script_dialog_confirm_set_confirmed(dialog, FALSE);
        break;
    case WEBKIT_SCRIPT_DIALOG_BEFORE_UNLOAD_CONFIRM:
        webkit_script_dialog_confirm_set_confirmed(dialog, TRUE);
        break;
    case WEBKIT_SCRIPT_DIALOG_PROMPT:
        webkit_script_dialog_prompt_set_text(dialog, NULL);
        break;
    default:
        break;
    }
    return TRUE;
}

static gboolean on_authenticate(WebKitWebView *view, WebKitAuthenticationRequest *request, Page *page)
{
    webkit_authentication_request_cancel(request);
    return TRUE;
}

static gboolean on_file_chooser(WebKitWebView *view, WebKitFileChooserRequest *request, Page *page)
{
    webkit_file_chooser_request_cancel(request);
    return TRUE;
}

static gboolean on_color_chooser(WebKitWebView *view, WebKitColorChooserRequest *request, Page *page)
{
    webkit_color_chooser_request_cancel(request);
    return TRUE;
}

static gboolean on_return_true(WebKitWebView *view, gpointer arg, Page *page)
{
    return TRUE;
}

/* ---------------------------------------------------------------------------------------------
 * Menus (PLT-015 R5 "M", CMP-022 R14)
 * ------------------------------------------------------------------------------------------ */

static void json_double_member(JsonBuilder *b, const char *name, double value)
{
    json_builder_set_member_name(b, name);
    json_builder_add_double_value(b, value);
}

static void menu_begin(JsonBuilder *b, double x, double y)
{
    json_double_member(b, "x", x);
    json_double_member(b, "y", y);
    json_builder_set_member_name(b, "items");
    json_builder_begin_array(b);
}

static void menu_item(JsonBuilder *b, const char *label, const char *action, gboolean enabled, gboolean selected)
{
    json_builder_begin_object(b);
    json_builder_set_member_name(b, "label");
    json_builder_add_string_value(b, label ? label : "");
    json_builder_set_member_name(b, "action");
    json_builder_add_string_value(b, action);
    json_builder_set_member_name(b, "enabled");
    json_builder_add_boolean_value(b, enabled);
    json_builder_set_member_name(b, "selected");
    json_builder_add_boolean_value(b, selected);
    json_builder_end_object(b);
}

static gboolean on_context_menu(WebKitWebView *view, WebKitContextMenu *menu, GdkEvent *event, WebKitHitTestResult *hit, Page *page)
{
    gboolean is_link = webkit_hit_test_result_context_is_link(hit);
    const char *link = is_link ? webkit_hit_test_result_get_link_uri(hit) : NULL;
    g_free(page->link);
    page->link = (link && *link) ? g_strdup(link) : NULL;
    gboolean editable = webkit_hit_test_result_context_is_editable(hit);
    gboolean selection = webkit_hit_test_result_context_is_selection(hit);

    JsonBuilder *b = vsp_json_begin();
    menu_begin(b, page->ptr_x, page->ptr_y);
    if (page->link) {
        menu_item(b, "Open link in new tab", "open-link", vsp_navigation_allowed(page->link), FALSE);
        menu_item(b, "Copy link address", "copy-link", TRUE, FALSE);
    }
    menu_item(b, "Copy", "copy", selection || editable, FALSE);
    if (editable)
        menu_item(b, "Paste", "text", TRUE, FALSE);
    menu_item(b, "Select all", "select-all", TRUE, FALSE);
    menu_item(b, "Back", "back", webkit_web_view_can_go_back(view), FALSE);
    menu_item(b, "Forward", "forward", webkit_web_view_can_go_forward(view), FALSE);
    menu_item(b, "Reload", "reload", TRUE, FALSE);
    json_builder_end_array(b);
    vsp_send_json_builder('M', page->id, b);
    return TRUE; /* the native menu is never shown */
}

static void option_menu_detach(Page *page)
{
    if (!page->option_menu)
        return;
    WebKitOptionMenu *menu = page->option_menu;
    page->option_menu = NULL;
    if (page->option_close_handler)
        g_signal_handler_disconnect(menu, page->option_close_handler);
    page->option_close_handler = 0;
    g_object_unref(menu);
}

static void on_option_menu_close(WebKitOptionMenu *menu, Page *page)
{
    if (page->option_menu == menu)
        option_menu_detach(page);
}

void vsp_page_close_option_menu(Page *page)
{
    if (!page->option_menu)
        return;
    WebKitOptionMenu *menu = g_object_ref(page->option_menu);
    option_menu_detach(page);
    webkit_option_menu_close(menu);
    g_object_unref(menu);
}

static gboolean on_show_option_menu(WebKitWebView *view, WebKitOptionMenu *menu, GdkEvent *event, GdkRectangle *rect, Page *page)
{
    vsp_page_close_option_menu(page);
    page->option_menu = g_object_ref(menu);
    page->option_close_handler = g_signal_connect(menu, "close", G_CALLBACK(on_option_menu_close), page);

    JsonBuilder *b = vsp_json_begin();
    double scale = page->scale;
    menu_begin(b, rect ? rect->x / scale : page->ptr_x, rect ? (rect->y + rect->height) / scale : page->ptr_y);
    guint count = webkit_option_menu_get_n_items(menu);
    for (guint i = 0; i < count; i++) {
        WebKitOptionMenuItem *item = webkit_option_menu_get_item(menu, i);
        char action[32];
        g_snprintf(action, sizeof action, "option:%u", i);
        gboolean enabled = webkit_option_menu_item_is_enabled(item) && !webkit_option_menu_item_is_group_label(item);
        menu_item(b, webkit_option_menu_item_get_label(item), action, enabled, webkit_option_menu_item_is_selected(item));
    }
    json_builder_end_array(b);
    vsp_send_json_builder('M', page->id, b);
    return TRUE;
}

/* "option:N": activate item N when in range, then close the menu. */
void vsp_page_option_command(Page *page, const char *cmd)
{
    if (!page->option_menu)
        return;
    const char *digits = cmd + strlen("option:");
    if (!*digits)
        return;
    gint64 index = 0;
    for (const char *p = digits; *p; p++) {
        if (*p < '0' || *p > '9' || index > G_MAXINT32)
            return;
        index = index * 10 + (*p - '0');
    }
    WebKitOptionMenu *menu = g_object_ref(page->option_menu);
    if ((guint64)index < webkit_option_menu_get_n_items(menu))
        webkit_option_menu_activate_item(menu, (guint)index);
    vsp_page_close_option_menu(page);
    g_object_unref(menu);
}

/* "open-link" / "copy-link" use the link captured by the last context menu. */
void vsp_page_link_command(Page *page, const char *cmd)
{
    if (!page->link)
        return;
    if (!strcmp(cmd, "open-link")) {
        if (vsp_navigation_allowed(page->link))
            vsp_send_text('N', page->id, page->link);
    } else {
        vsp_send_text('C', page->id, page->link);
    }
}

/* ---------------------------------------------------------------------------------------------
 * Damage tracking of the offscreen window (PLT-015 R7)
 * ------------------------------------------------------------------------------------------ */

static gboolean on_damage_event(GtkWidget *widget, GdkEventExpose *event, Page *page)
{
    vsp_frame_mark_dirty(page);
    return FALSE;
}

/* ---------------------------------------------------------------------------------------------
 * Lifecycle
 * ------------------------------------------------------------------------------------------ */

static void apply_geometry(Page *page)
{
    webkit_web_view_set_zoom_level(page->view, page->scale);
    gtk_window_set_default_size(GTK_WINDOW(page->window), page->width, page->height);
    gtk_widget_set_size_request(GTK_WIDGET(page->view), page->width, page->height);
    gtk_window_resize(GTK_WINDOW(page->window), page->width, page->height);
    gtk_widget_queue_resize(page->window);
}

Page *vsp_page_create(guint32 id)
{
    if (id == 0 || vsp_page_lookup(id))
        return NULL;
    Page *page = g_new0(Page, 1);
    page->id = id;
    page->width = VSP_DEFAULT_WIDTH;
    page->height = VSP_DEFAULT_HEIGHT;
    page->scale = 1.0;
    page->visible = TRUE;
    page->dirty = TRUE;
    page->full_damage = TRUE;

    page->window = gtk_offscreen_window_new();
    page->view = WEBKIT_WEB_VIEW(g_object_new(WEBKIT_TYPE_WEB_VIEW, "web-context", vsp_web_context, "settings", settings, "user-content-manager", content_manager, NULL));
    g_object_ref_sink(page->view);
    gtk_container_add(GTK_CONTAINER(page->window), GTK_WIDGET(page->view));
    g_object_unref(page->view); /* the container owns it now */

    page->im = vsp_im_new(id);
    webkit_web_view_set_input_method_context(page->view, page->im);

    g_signal_connect(page->view, "notify::uri", G_CALLBACK(on_state_notify), page);
    g_signal_connect(page->view, "notify::title", G_CALLBACK(on_state_notify), page);
    g_signal_connect(page->view, "notify::is-loading", G_CALLBACK(on_loading_notify), page);
    g_signal_connect(page->view, "load-changed", G_CALLBACK(on_load_changed), page);
    g_signal_connect(page->view, "load-failed", G_CALLBACK(on_load_failed), page);
    g_signal_connect(page->view, "web-process-terminated", G_CALLBACK(on_web_process_terminated), page);
    g_signal_connect(page->view, "decide-policy", G_CALLBACK(on_decide_policy), page);
    g_signal_connect(page->view, "permission-request", G_CALLBACK(on_permission_request), page);
    g_signal_connect(page->view, "query-permission-state", G_CALLBACK(on_query_permission_state), page);
    g_signal_connect(page->view, "script-dialog", G_CALLBACK(on_script_dialog), page);
    g_signal_connect(page->view, "authenticate", G_CALLBACK(on_authenticate), page);
    g_signal_connect(page->view, "run-file-chooser", G_CALLBACK(on_file_chooser), page);
    g_signal_connect(page->view, "run-color-chooser", G_CALLBACK(on_color_chooser), page);
    g_signal_connect(page->view, "show-notification", G_CALLBACK(on_return_true), page);
    g_signal_connect(page->view, "print", G_CALLBACK(on_return_true), page);
    g_signal_connect(page->view, "create", G_CALLBACK(on_create), page);
    g_signal_connect(page->view, "context-menu", G_CALLBACK(on_context_menu), page);
    g_signal_connect(page->view, "show-option-menu", G_CALLBACK(on_show_option_menu), page);
    g_signal_connect(page->window, "damage-event", G_CALLBACK(on_damage_event), page);

    g_hash_table_insert(pages, GUINT_TO_POINTER(id), page);
    apply_geometry(page);
    gtk_widget_show_all(page->window);
    /* WebKit reads the widget's scale factor at construction, before the view is inside the
     * offscreen window: on Wayland that is the output's scale (devicePixelRatio would be the
     * output scale times the page scale although GDK scaling is meant to be 1). Re-announcing the
     * property makes the engine pick up the window's real scale factor. */
    g_object_notify(G_OBJECT(page->view), "scale-factor");
    return page;
}

void vsp_page_close(guint32 id)
{
    Page *page = vsp_page_lookup(id);
    if (!page)
        return;
    vsp_page_close_option_menu(page);
    g_signal_handlers_disconnect_by_data(page->view, page);
    g_signal_handlers_disconnect_by_data(page->window, page);
    vsp_frame_page_free(page);
    g_clear_object(&page->im);
    gtk_widget_destroy(page->window);
    g_hash_table_remove(pages, GUINT_TO_POINTER(id));
}

void vsp_pages_foreach(GHFunc fn, gpointer data)
{
    g_hash_table_foreach(pages, fn, data);
}

void vsp_pages_close_all(void)
{
    GList *ids = g_hash_table_get_keys(pages);
    for (GList *l = ids; l; l = l->next)
        vsp_page_close(GPOINTER_TO_UINT(l->data));
    g_list_free(ids);
}

void vsp_page_load(Page *page, const char *url)
{
    if (!vsp_navigation_allowed(url)) {
        g_free(page->error);
        page->error = g_strdup("Only http and https addresses are supported.");
        vsp_page_send_state(page);
        return;
    }
    webkit_web_view_load_uri(page->view, url);
}

void vsp_page_resize(Page *page, int width, int height, double scale)
{
    width = CLAMP(width, 1, VSP_MAX_DIM);
    height = CLAMP(height, 1, VSP_MAX_DIM);
    scale = CLAMP(scale, VSP_MIN_SCALE, VSP_MAX_SCALE);
    if (width == page->width && height == page->height && scale == page->scale)
        return;
    page->width = width;
    page->height = height;
    page->scale = scale;
    apply_geometry(page);
    vsp_frame_mark_full(page);
}

void vsp_page_set_visible(Page *page, gboolean visible)
{
    if (visible == page->visible)
        return;
    page->visible = visible;
    if (visible) {
        gtk_widget_show(page->window);
        vsp_frame_mark_full(page);
    } else {
        gtk_widget_hide(page->window);
    }
}

static void on_eval_done(GObject *source, GAsyncResult *result, gpointer data)
{
    guint32 id = GPOINTER_TO_UINT(data);
    GError *error = NULL;
    JSCValue *value = webkit_web_view_evaluate_javascript_finish(WEBKIT_WEB_VIEW(source), result, &error);
    g_clear_error(&error);
    Page *page = vsp_page_lookup(id);
    if (page && !vsp_shutting_down) {
        char *json = value ? jsc_value_to_json(value, 0) : NULL;
        vsp_send_text('J', id, json ? json : "null");
        g_free(json);
    }
    if (value)
        g_object_unref(value);
}

void vsp_page_eval(Page *page, const char *script)
{
    webkit_web_view_evaluate_javascript(page->view, script, -1, NULL, NULL, NULL, on_eval_done, GUINT_TO_POINTER(page->id));
}

void vsp_pages_init(void)
{
    pages = g_hash_table_new_full(g_direct_hash, g_direct_equal, NULL, page_free);
    /* Two things the engine does not route through the navigation policy are handled by a script
     * in an isolated world (all frames, document start):
     *  - Activating an anchor, area or form whose target is a javascript: URL runs the script
     *    without any policy decision (PLT-014 R10: such navigations are ignored). Script
     *    activation is cancelled in the capture phase; ordinary page scripts are unaffected.
     *  - Releasing the middle button makes the engine paste the PRIMARY selection (PLT-016 R17:
     *    not provided), which crashes the helper because the offscreen window never has input
     *    focus. The release is marked as handled; the page still receives press, release and
     *    auxclick. */
    static const char *guard_script =
        "(function(){"
        "function js(u){try{return new URL(u,document.baseURI).protocol==='javascript:';}catch(e){return false;}}"
        "function link(t){return t&&t.closest?t.closest('a[href],area[href]'):null;}"
        "function block(e){"
        "var t=e.target,a=link(t);"
        "if(a&&js(a.href)){e.preventDefault();e.stopImmediatePropagation();return;}"
        "var b=t&&t.closest?t.closest('button,input'):null;"
        "if(b&&b.form&&(b.type==='submit'||b.type==='image')&&js(b.formAction)){e.preventDefault();e.stopImmediatePropagation();}"
        "}"
        "window.addEventListener('click',block,true);"
        "window.addEventListener('auxclick',block,true);"
        "window.addEventListener('submit',function(e){var f=e.target;"
        "if(f&&f.tagName==='FORM'&&(js(f.action)||(e.submitter&&e.submitter.formAction&&js(e.submitter.formAction)))){e.preventDefault();e.stopImmediatePropagation();}},true);"
        "window.addEventListener('mouseup',function(e){if(e.button===1)e.preventDefault();},true);"
        "})();";
    content_manager = webkit_user_content_manager_new();
    WebKitUserScript *script = webkit_user_script_new_for_world(
        guard_script, WEBKIT_USER_CONTENT_INJECT_ALL_FRAMES, WEBKIT_USER_SCRIPT_INJECT_AT_DOCUMENT_START, "vsp-helper", NULL, NULL);
    webkit_user_content_manager_add_script(content_manager, script);
    webkit_user_script_unref(script);
    settings = webkit_settings_new();
    webkit_settings_set_enable_developer_extras(settings, FALSE);
    g_signal_connect(vsp_web_context, "download-started", G_CALLBACK(on_download_started), NULL);
}
