#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <strings.h>
#include <netdb.h>
#include <dlfcn.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>
#include <pthread.h>
#include <ctype.h>

#define SHIELD_VERSION "0.1.0"
#define MAX_RULES 32768
#define RELOAD_THROTTLE_SECS 1

static int (*real_getaddrinfo)(const char *node, const char *service,
                               const struct addrinfo *hints,
                               struct addrinfo **res) = NULL;

typedef struct {
    char **rules;
    size_t count;
    time_t last_mtime;
    time_t last_check;
} RuleTable;

static RuleTable g_table = { NULL, 0, 0, 0 };
static pthread_mutex_t g_mutex = PTHREAD_MUTEX_INITIALIZER;
static char g_rules_path[1024] = {0};
static int g_path_initialized = 0;

static void init_rules_path(void) {
    if (g_path_initialized) return;

    // 1. Check $LVR_SHIELD_RULES
    const char *custom = getenv("LVR_SHIELD_RULES");
    if (custom && custom[0]) {
        snprintf(g_rules_path, sizeof(g_rules_path), "%s", custom);
        g_path_initialized = 1;
        return;
    }

    // 2. Check XDG_RUNTIME_DIR (/run/user/<uid>/lvr/shield_rules.txt)
    const char *runtime = getenv("XDG_RUNTIME_DIR");
    if (runtime && runtime[0]) {
        snprintf(g_rules_path, sizeof(g_rules_path), "%s/lvr/shield_rules.txt", runtime);
        struct stat st;
        if (stat(g_rules_path, &st) == 0) {
            g_path_initialized = 1;
            return;
        }
    }

    // 3. Fall back to ~/.cache/lvr/shield_rules.txt
    const char *home = getenv("HOME");
    if (home && home[0]) {
        snprintf(g_rules_path, sizeof(g_rules_path), "%s/.cache/lvr/shield_rules.txt", home);
    } else {
        snprintf(g_rules_path, sizeof(g_rules_path), "/tmp/lvr_shield_rules.txt");
    }
    g_path_initialized = 1;
}

static void free_rules_unlocked(void) {
    if (g_table.rules) {
        for (size_t i = 0; i < g_table.count; i++) {
            free(g_table.rules[i]);
        }
        free(g_table.rules);
        g_table.rules = NULL;
    }
    g_table.count = 0;
}

// Load simple newline-separated rules list (.txt)
static void load_rules_unlocked(const char *path) {
    FILE *f = fopen(path, "r");
    if (!f) return;

    char **new_rules = malloc(sizeof(char*) * MAX_RULES);
    if (!new_rules) {
        fclose(f);
        return;
    }

    size_t count = 0;
    char line[512];

    while (fgets(line, sizeof(line), f) && count < MAX_RULES) {
        // Strip comments (#)
        char *comment = strchr(line, '#');
        if (comment) *comment = '\0';

        // Trim leading whitespace
        char *start = line;
        while (*start && isspace((unsigned char)*start)) start++;

        // Trim trailing whitespace
        char *end = start + strlen(start);
        while (end > start && isspace((unsigned char)*(end - 1))) {
            end--;
            *end = '\0';
        }

        if (*start) {
            // Lowercase
            for (char *c = start; *c; c++) *c = tolower((unsigned char)*c);
            new_rules[count] = strdup(start);
            if (new_rules[count]) {
                count++;
            }
        }
    }

    fclose(f);

    free_rules_unlocked();
    g_table.rules = new_rules;
    g_table.count = count;
}

static void check_and_reload_rules(void) {
    init_rules_path();

    time_t now = time(NULL);
    if (now - g_table.last_check < RELOAD_THROTTLE_SECS && g_table.last_check != 0) {
        return;
    }

    pthread_mutex_lock(&g_mutex);
    g_table.last_check = now;

    struct stat st;
    if (stat(g_rules_path, &st) == 0) {
        if (st.st_mtime != g_table.last_mtime) {
            load_rules_unlocked(g_rules_path);
            g_table.last_mtime = st.st_mtime;
        }
    } else {
        // If file disappeared or does not exist yet, clear rules
        if (g_table.count > 0) {
            free_rules_unlocked();
            g_table.last_mtime = 0;
        }
    }
    pthread_mutex_unlock(&g_mutex);
}

// Matches a domain against an exact rule or wildcard rule
// e.g. rule = "*.github.io", node = "test.github.io" -> match
//      rule = "youtube.com", node = "youtube.com" -> match
static int match_rule(const char *node, size_t node_len, const char *rule) {
    size_t rule_len = strlen(rule);

    if (rule[0] == '*' && rule[1] == '.') {
        const char *base = rule + 2;
        size_t base_len = rule_len - 2;

        // node == base ("github.io")
        if (node_len == base_len) {
            return strcasecmp(node, base) == 0;
        }
        // node ends with ".base" ("sub.github.io")
        if (node_len > base_len && node[node_len - base_len - 1] == '.') {
            return strcasecmp(node + (node_len - base_len), base) == 0;
        }
        return 0;
    }

    // Exact match
    if (node_len == rule_len) {
        return strcasecmp(node, rule) == 0;
    }

    return 0;
}

static int is_domain_blocked(const char *node) {
    if (!node || !node[0]) return 0;

    check_and_reload_rules();

    if (g_table.count == 0) return 0;

    size_t node_len = strlen(node);

    pthread_mutex_lock(&g_mutex);
    for (size_t i = 0; i < g_table.count; i++) {
        if (g_table.rules[i] && match_rule(node, node_len, g_table.rules[i])) {
            pthread_mutex_unlock(&g_mutex);
            return 1;
        }
    }
    pthread_mutex_unlock(&g_mutex);

    return 0;
}

int getaddrinfo(const char *node, const char *service,
                const struct addrinfo *hints,
                struct addrinfo **res) {
    if (!real_getaddrinfo) {
        real_getaddrinfo = dlsym(RTLD_NEXT, "getaddrinfo");
        if (!real_getaddrinfo) {
            // Absolute failsafe: cannot resolve original getaddrinfo
            return EAI_SYSTEM;
        }
    }

    // Always passthrough if node is NULL (e.g. AI_PASSIVE listening sockets)
    if (!node) {
        return real_getaddrinfo(node, service, hints, res);
    }

    // Check if domain is blocked by active LVR rules
    if (is_domain_blocked(node)) {
        // Return 0.0.0.0 via standard getaddrinfo to provide valid sockaddr with 0.0.0.0
        int ret = real_getaddrinfo("0.0.0.0", service, hints, res);
        if (ret == 0) {
            return 0;
        }
        // Failsafe if 0.0.0.0 lookup fails
        return EAI_NONAME;
    }

    // Guaranteed fallback: pass through directly to libc getaddrinfo
    return real_getaddrinfo(node, service, hints, res);
}
