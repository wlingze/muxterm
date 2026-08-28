#include <signal.h>
#include <stdio.h>
#include <sys/ioctl.h>
#include <unistd.h>

// macOS App E2E 的真实进程夹具：可执行文件名必须是 `pi`，保持在
// primary screen，不开启 mouse/alternate-screen，并制造大量原地重绘历史。
// 这样 tmux 的 pane_current_command/mode 与 1320 日志里的真实 pi 一致。

static volatile sig_atomic_t needs_redraw = 0;

static void handle_winch(int signal_number) {
    (void)signal_number;
    needs_redraw = 1;
}

static int terminal_rows(void) {
    struct winsize size;
    if (ioctl(STDOUT_FILENO, TIOCGWINSZ, &size) == 0 && size.ws_row >= 8) {
        return size.ws_row;
    }
    return 24;
}

static void draw_screen(void) {
    const int rows = terminal_rows();
    printf("\033[H\033[2J");
    printf("PI_E2E_HEADER primary-no-mouse\r\n");
    printf("PI_E2E_BODY current-agent-message\r\n");
    printf("\033[%d;1HPI_E2E_PROMPT > ", rows - 2);
    fflush(stdout);
}

int main(void) {
    setvbuf(stdout, NULL, _IONBF, 0);
    signal(SIGWINCH, handle_winch);

    // OSC 133/9;4 与 SGR/CUP 模拟 pi 的富历史。它们是历史网格，不是可以
    // 再喂给前端 VT 的原始 PTY 流。
    for (int i = 0; i < 320; ++i) {
        printf("\033]133;B\007\033]133;C\007");
        printf("\033[3%dmPI_E2E_HISTORY_%03d previous-agent-frame\033[0m\r\n", i % 8, i);
        printf("\033]9;4;3\007");
    }
    draw_screen();

    for (;;) {
        if (needs_redraw) {
            needs_redraw = 0;
            draw_screen();
            continue;
        }
        pause();
    }
}
