/* A PipeWire client that presents itself the way xdg-desktop-portal presents
 * an app it has brokered a camera for, and prints every registry add/remove it
 * sees, with a millisecond timestamp.
 *
 * This exists because no shipped pipewire tool can be made to do it:
 * pw-dump / pw-mon / pw-cli all connect with remote.intention = manager and so
 * land on pipewire-0-manager, which module-access maps to "unrestricted", and
 * PIPEWIRE_PROPS does not reach client properties in any syntax. The
 * connection properties are the whole point of the measurement, so they have
 * to be set by a client that sets them itself.
 *
 * Prints one line per event, so the caller can time a revocation against it.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <signal.h>

#include <pipewire/pipewire.h>

static struct pw_main_loop *loop;

static double now_ms(void)
{
	struct timespec ts;
	clock_gettime(CLOCK_MONOTONIC, &ts);
	return ts.tv_sec * 1000.0 + ts.tv_nsec / 1e6;
}

static void on_global(void *data, uint32_t id, uint32_t permissions,
		      const char *type, uint32_t version,
		      const struct spa_dict *props)
{
	const char *name = props ? spa_dict_lookup(props, "node.name") : NULL;
	const char *cls = props ? spa_dict_lookup(props, "media.class") : NULL;
	printf("%.1f ADD id=%u type=%s name=%s class=%s\n",
	       now_ms(), id, type, name ? name : "-", cls ? cls : "-");
	fflush(stdout);
}

static void on_global_remove(void *data, uint32_t id)
{
	printf("%.1f REMOVE id=%u\n", now_ms(), id);
	fflush(stdout);
}

static const struct pw_registry_events registry_events = {
	PW_VERSION_REGISTRY_EVENTS,
	.global = on_global,
	.global_remove = on_global_remove,
};

static void on_core_info(void *data, const struct pw_core_info *info)
{
	printf("%.1f CORE cookie=%u\n", now_ms(), info->cookie);
	fflush(stdout);
}

static void on_core_error(void *data, uint32_t id, int seq, int res, const char *message)
{
	printf("%.1f ERROR id=%u res=%d %s\n", now_ms(), id, res, message);
	fflush(stdout);
}

static const struct pw_core_events core_events = {
	PW_VERSION_CORE_EVENTS,
	.info = on_core_info,
	.error = on_core_error,
};

static void on_sigint(void *data, int signal_number)
{
	pw_main_loop_quit(loop);
}

int main(int argc, char *argv[])
{
	struct pw_context *context;
	struct pw_core *core;
	struct pw_registry *registry;
	struct spa_hook registry_listener = { 0 };
	struct spa_hook core_listener = { 0 };
	const char *app_id = argc > 1 ? argv[1] : "com.example.apextest";
	const char *remote = argc > 2 ? argv[2] : "pipewire-0";

	pw_init(&argc, &argv);

	loop = pw_main_loop_new(NULL);
	pw_loop_add_signal(pw_main_loop_get_loop(loop), SIGINT, on_sigint, NULL);
	pw_loop_add_signal(pw_main_loop_get_loop(loop), SIGTERM, on_sigint, NULL);

	context = pw_context_new(pw_main_loop_get_loop(loop), NULL, 0);
	if (context == NULL) {
		fprintf(stderr, "no context\n");
		return 1;
	}

	/* Exactly the properties xdg-desktop-portal sets on the connection it
	 * hands an app after AccessCamera: the app's Flatpak id, the media
	 * roles it was granted, and is_portal=no to say this connection is the
	 * APP's rather than the portal's own. `pipewire.client.access` is the
	 * client asking to be treated as a portal client; module-access decides
	 * whether to honour it. Deliberately NO remote.intention, so this lands
	 * on the plain socket rather than the manager one. */
	struct pw_properties *props = pw_properties_new(
		PW_KEY_REMOTE_NAME, remote,
		PW_KEY_APP_NAME, "apex-portal-client",
		"pipewire.client.access", "portal",
		"pipewire.access.portal.app_id", app_id,
		"pipewire.access.portal.media_roles", "Camera",
		"pipewire.access.portal.is_portal", "no",
		NULL);

	core = pw_context_connect(context, props, 0);
	if (core == NULL) {
		fprintf(stderr, "could not connect: %m\n");
		return 1;
	}
	pw_core_add_listener(core, &core_listener, &core_events, NULL);

	registry = pw_core_get_registry(core, PW_VERSION_REGISTRY, 0);
	pw_registry_add_listener(registry, &registry_listener, &registry_events, NULL);

	printf("%.1f READY app_id=%s remote=%s\n", now_ms(), app_id, remote);
	fflush(stdout);

	pw_main_loop_run(loop);

	pw_proxy_destroy((struct pw_proxy *) registry);
	pw_core_disconnect(core);
	pw_context_destroy(context);
	pw_main_loop_destroy(loop);
	pw_deinit();
	return 0;
}
