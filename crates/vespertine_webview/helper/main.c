/* Vespertine embedded web view helper: GTK 3 + WebKitGTK 4.1 + json-glib, one WebKitWebView per
 * page inside a GtkOffscreenWindow, controlled over a framed AF_UNIX channel (PLT-014/015/016). */
#include "vsp.h"

#include <errno.h>
#include <signal.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

GMainLoop *vsp_main_loop = NULL;

static int usage(const char *message)
{
    fprintf(stderr, "vsp-webkit-helper: %s\n", message);
    return 2;
}

static char *engine_string(void)
{
    return g_strdup_printf("WebKitGTK %u.%u.%u", webkit_get_major_version(), webkit_get_minor_version(), webkit_get_micro_version());
}

static void send_ready(void)
{
    char *engine = engine_string();
    JsonBuilder *b = vsp_json_begin();
    json_builder_set_member_name(b, "protocol");
    json_builder_add_int_value(b, VSP_PROTOCOL_VERSION);
    json_builder_set_member_name(b, "engine");
    json_builder_add_string_value(b, engine);
    json_builder_set_member_name(b, "transfers");
    json_builder_begin_array(b);
    json_builder_add_string_value(b, "inline");
    json_builder_add_string_value(b, "shm");
    json_builder_end_array(b);
    json_builder_set_member_name(b, "capabilities");
    json_builder_begin_array(b);
    json_builder_add_string_value(b, "ping");
    json_builder_add_string_value(b, "eval");
    json_builder_end_array(b);
    vsp_send_json_builder('R', 0, b);
    g_free(engine);
}

int main(int argc, char **argv)
{
    /* PLT-014 R2: environment before GTK initialization. */
    setenv("WEBKIT_DISABLE_DMABUF_RENDERER", "1", 1);
    setenv("GDK_SCALE", "1", 1);
    setenv("GDK_DPI_SCALE", "1", 1);
    signal(SIGPIPE, SIG_IGN);
    umask(0077);

    if (argc < 3 || strcmp(argv[1], "--channel") != 0)
        return usage("usage: vsp-webkit-helper --channel <fd> [--profile <absolute directory>]");
    char *end = NULL;
    errno = 0;
    long fd = strtol(argv[2], &end, 10);
    if (errno || !end || *end || fd < 3 || fd > 65535)
        return usage("invalid channel descriptor");
    if (!vsp_channel_open((int)fd))
        return usage("the channel descriptor is not usable");

    /* PLT-014 R1: no message may be sent before GTK has a display. */
    int gtk_argc = 1;
    char *gtk_args[] = { argv[0], NULL };
    char **gtk_argv = gtk_args;
    if (!gtk_init_check(&gtk_argc, &gtk_argv)) {
        fprintf(stderr, "vsp-webkit-helper: cannot connect to a display (Wayland or X11)\n");
        return 1;
    }

    int exit_code = 1;
    vsp_web_context = vsp_profile_open(argc, argv, 3, &exit_code);
    if (!vsp_web_context)
        return exit_code;

    vsp_main_loop = g_main_loop_new(NULL, FALSE);
    vsp_pages_init();
    vsp_input_init();
    vsp_frames_start();
    send_ready();

    g_main_loop_run(vsp_main_loop);

    /* Release the profile lock (closed with the process) and leave. */
    vsp_channel_close();
    fflush(stderr);
    _exit(0);
}
