/* Profile storage and the engine context (PLT-014 R8). */
#include "vsp.h"

#include <errno.h>
#include <fcntl.h>
#include <string.h>
#include <sys/file.h>
#include <sys/stat.h>
#include <unistd.h>

static void fail(const char *message, int *exit_code)
{
    vsp_send_text('E', 0, message);
    *exit_code = 1;
}

static gboolean ensure_dir(const char *path)
{
    if (g_mkdir_with_parents(path, 0700) != 0)
        return FALSE;
    struct stat st;
    return stat(path, &st) == 0 && S_ISDIR(st.st_mode);
}

WebKitWebContext *vsp_profile_open(int argc, char **argv, int first_arg, int *exit_code)
{
    int rest = argc - first_arg;
    if (rest == 0)
        return webkit_web_context_new_ephemeral();

    if (rest != 2 || strcmp(argv[first_arg], "--profile") != 0 || argv[first_arg + 1][0] != '/') {
        fail("Browser profile requires an absolute storage directory", exit_code);
        return NULL;
    }
    const char *root = argv[first_arg + 1];

    if (!ensure_dir(root)) {
        fail("Could not create or write the browser profile directory", exit_code);
        return NULL;
    }
    {
        char *probe = g_strdup_printf("%s/.vsp-probe-%d", root, (int)getpid());
        int fd = open(probe, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC, 0600);
        gboolean ok = fd >= 0;
        if (fd >= 0) {
            close(fd);
            ok = unlink(probe) == 0;
        }
        g_free(probe);
        if (!ok) {
            fail("Could not create or write the browser profile directory", exit_code);
            return NULL;
        }
    }

    /* The lock descriptor stays open (and the lock held) for the helper's lifetime. */
    {
        char *lock_path = g_build_filename(root, "profile.lock", NULL);
        int lock_fd = open(lock_path, O_RDWR | O_CREAT | O_CLOEXEC, 0600);
        g_free(lock_path);
        if (lock_fd < 0 || flock(lock_fd, LOCK_EX | LOCK_NB) != 0) {
            fail("Browser profile is in use by another process or cannot be locked. Close the other "
                 "instance and reopen this tab.",
                 exit_code);
            return NULL;
        }
    }

    char *data_dir = g_build_filename(root, "data", NULL);
    char *cache_dir = g_build_filename(root, "cache", NULL);
    char *cookies = g_build_filename(root, "cookies.sqlite", NULL);
    int saved_errno = 0;
    gboolean ok = TRUE;
    if (!ensure_dir(data_dir) || !ensure_dir(cache_dir)) {
        saved_errno = errno ? errno : EIO;
        ok = FALSE;
    } else {
        int fd = open(cookies, O_RDWR | O_CREAT | O_CLOEXEC, 0600);
        if (fd < 0) {
            saved_errno = errno;
            ok = FALSE;
        } else {
            close(fd);
        }
    }
    if (!ok) {
        char *message = g_strdup_printf("Could not open browser profile: %s", g_strerror(saved_errno));
        fail(message, exit_code);
        g_free(message);
        g_free(data_dir);
        g_free(cache_dir);
        g_free(cookies);
        return NULL;
    }

    WebKitWebsiteDataManager *manager = webkit_website_data_manager_new(
        "base-data-directory", data_dir, "base-cache-directory", cache_dir, NULL);
    WebKitWebContext *context = webkit_web_context_new_with_website_data_manager(manager);
    WebKitCookieManager *cookie_manager = webkit_website_data_manager_get_cookie_manager(manager);
    webkit_cookie_manager_set_persistent_storage(cookie_manager, cookies, WEBKIT_COOKIE_PERSISTENT_STORAGE_SQLITE);
    g_object_unref(manager);
    g_free(data_dir);
    g_free(cache_dir);
    g_free(cookies);
    return context;
}
