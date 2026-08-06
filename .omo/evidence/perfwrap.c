#define _GNU_SOURCE
#include <errno.h>
#include <inttypes.h>
#include <linux/perf_event.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

struct event_spec {
    const char *name;
    uint32_t type;
    uint64_t config;
    int fd;
    uint64_t id;
};

static long open_event(struct perf_event_attr *attr, pid_t pid, int group_fd) {
    return syscall(SYS_perf_event_open, attr, pid, -1, group_fd, 0);
}

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr, "usage: perfwrap COMMAND [ARGS...]\n");
        return 2;
    }

    struct event_spec events[] = {
        {"cycles", PERF_TYPE_HARDWARE, PERF_COUNT_HW_CPU_CYCLES, -1, 0},
        {"instructions", PERF_TYPE_HARDWARE, PERF_COUNT_HW_INSTRUCTIONS, -1, 0},
        {"branches", PERF_TYPE_HARDWARE, PERF_COUNT_HW_BRANCH_INSTRUCTIONS, -1, 0},
        {"branch_misses", PERF_TYPE_HARDWARE, PERF_COUNT_HW_BRANCH_MISSES, -1, 0},
        {"cache_misses", PERF_TYPE_HARDWARE, PERF_COUNT_HW_CACHE_MISSES, -1, 0},
        {"context_switches", PERF_TYPE_SOFTWARE, PERF_COUNT_SW_CONTEXT_SWITCHES, -1, 0},
        {"cpu_migrations", PERF_TYPE_SOFTWARE, PERF_COUNT_SW_CPU_MIGRATIONS, -1, 0},
    };
    const size_t event_count = sizeof(events) / sizeof(events[0]);

    pid_t child = fork();
    if (child < 0) {
        perror("fork");
        return 2;
    }
    if (child == 0) {
        raise(SIGSTOP);
        execvp(argv[1], &argv[1]);
        perror("execvp");
        _exit(127);
    }

    int status = 0;
    if (waitpid(child, &status, WUNTRACED) < 0 || !WIFSTOPPED(status)) {
        perror("waitpid stopped child");
        kill(child, SIGKILL);
        return 2;
    }

    int leader_fd = -1;
    for (size_t index = 0; index < event_count; ++index) {
        struct perf_event_attr attr;
        memset(&attr, 0, sizeof(attr));
        attr.type = events[index].type;
        attr.size = sizeof(attr);
        attr.config = events[index].config;
        attr.disabled = index == 0;
        attr.inherit = 1;
        attr.exclude_kernel = 1;
        attr.exclude_hv = 1;
        attr.read_format = PERF_FORMAT_GROUP | PERF_FORMAT_ID |
                           PERF_FORMAT_TOTAL_TIME_ENABLED |
                           PERF_FORMAT_TOTAL_TIME_RUNNING;
        int group_fd = index == 0 ? -1 : leader_fd;
        events[index].fd = (int)open_event(&attr, child, group_fd);
        if (events[index].fd < 0) {
            fprintf(stderr, "perf_event_open %s: %s\n", events[index].name,
                    strerror(errno));
            kill(child, SIGKILL);
            waitpid(child, NULL, 0);
            return 3;
        }
        if (index == 0) {
            leader_fd = events[index].fd;
        }
        if (ioctl(events[index].fd, PERF_EVENT_IOC_ID, &events[index].id) != 0) {
            perror("PERF_EVENT_IOC_ID");
            kill(child, SIGKILL);
            waitpid(child, NULL, 0);
            return 3;
        }
    }

    ioctl(leader_fd, PERF_EVENT_IOC_RESET, PERF_IOC_FLAG_GROUP);
    ioctl(leader_fd, PERF_EVENT_IOC_ENABLE, PERF_IOC_FLAG_GROUP);
    kill(child, SIGCONT);
    if (waitpid(child, &status, 0) < 0) {
        perror("waitpid child exit");
        return 2;
    }
    ioctl(leader_fd, PERF_EVENT_IOC_DISABLE, PERF_IOC_FLAG_GROUP);

    struct {
        uint64_t nr;
        uint64_t time_enabled;
        uint64_t time_running;
        struct {
            uint64_t value;
            uint64_t id;
        } values[16];
    } result;
    memset(&result, 0, sizeof(result));
    if (read(leader_fd, &result, sizeof(result)) < 0) {
        perror("read perf group");
        return 3;
    }

    printf("perf_counts time_enabled=%" PRIu64 " time_running=%" PRIu64,
           result.time_enabled, result.time_running);
    for (size_t index = 0; index < event_count; ++index) {
        uint64_t value = 0;
        for (uint64_t slot = 0; slot < result.nr; ++slot) {
            if (result.values[slot].id == events[index].id) {
                value = result.values[slot].value;
                break;
            }
        }
        long double scaled = value;
        if (result.time_running > 0 && result.time_running < result.time_enabled) {
            scaled *= (long double)result.time_enabled / result.time_running;
        }
        printf(" %s=%.0Lf", events[index].name, scaled);
    }
    printf("\n");

    for (size_t index = 0; index < event_count; ++index) {
        close(events[index].fd);
    }
    if (WIFEXITED(status)) {
        return WEXITSTATUS(status);
    }
    return 128 + WTERMSIG(status);
}
