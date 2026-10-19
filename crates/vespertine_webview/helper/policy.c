/* Scheme allow-list (PLT-014 R10; the same rule as CMP-022 R5). */
#include <string.h>
#include "vsp.h"

static GUri *parse(const char *uri)
{
    if (!uri || !*uri)
        return NULL;
    return g_uri_parse(uri, G_URI_FLAGS_PARSE_RELAXED | G_URI_FLAGS_HAS_PASSWORD | G_URI_FLAGS_ENCODED, NULL);
}

static gboolean web_uri_ok(GUri *parsed)
{
    const char *scheme = g_uri_get_scheme(parsed);
    if (!scheme || (g_ascii_strcasecmp(scheme, "http") != 0 && g_ascii_strcasecmp(scheme, "https") != 0))
        return FALSE;
    const char *host = g_uri_get_host(parsed);
    if (!host || !*host)
        return FALSE;
    const char *user = g_uri_get_user(parsed);
    const char *password = g_uri_get_password(parsed);
    if ((user && *user) || (password && *password))
        return FALSE;
    const char *userinfo = g_uri_get_userinfo(parsed);
    if (userinfo && *userinfo && strcmp(userinfo, ":") != 0)
        return FALSE;
    return TRUE;
}

/* Top-level navigation: http(s) with a host and no credentials. */
gboolean vsp_navigation_allowed(const char *uri)
{
    GUri *parsed = parse(uri);
    if (!parsed)
        return FALSE;
    gboolean ok = web_uri_ok(parsed);
    g_uri_unref(parsed);
    return ok;
}

/* Sub-frame navigation: additionally the documents of the page itself (about:, data:, blob:). */
gboolean vsp_frame_navigation_allowed(const char *uri)
{
    GUri *parsed = parse(uri);
    if (!parsed)
        return FALSE;
    const char *scheme = g_uri_get_scheme(parsed);
    gboolean ok = web_uri_ok(parsed);
    if (!ok && scheme)
        ok = !g_ascii_strcasecmp(scheme, "about") || !g_ascii_strcasecmp(scheme, "data") ||
             !g_ascii_strcasecmp(scheme, "blob");
    g_uri_unref(parsed);
    return ok;
}
