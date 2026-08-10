#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdlib.h>

typedef void (*QGuiApplicationConstructor)(void *, int *, char **, int);

__attribute__((constructor)) static void clear_preload_from_environment(void) {
    unsetenv("LD_PRELOAD");
}

/* Quickshell 0.3.0 passes argc=0; Qt WebEngine requires argv[0]. */
void qgui_application_constructor(void *, int *, char **, int)
    __asm__("_ZN15QGuiApplicationC1ERiPPci");

void qgui_application_constructor(void *self, int *argc, char **argv, int flags) {
    static QGuiApplicationConstructor real_constructor;

    if (!real_constructor)
        real_constructor = (QGuiApplicationConstructor)dlsym(
            RTLD_NEXT, "_ZN15QGuiApplicationC1ERiPPci");

    if (*argc == 0)
        *argc = 1;

    real_constructor(self, argc, argv, flags);
}
