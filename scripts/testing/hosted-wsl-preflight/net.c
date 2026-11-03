// Test-only nonce listener/client; no Splitty implementation or protocol.
#include <arpa/inet.h>
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc != 4 || (strcmp(argv[1], "serve") && strcmp(argv[1], "check"))) return 64;
    char *end; long port = strtol(argv[2], &end, 10);
    if (*end || port < 1024 || port > 65535 || strlen(argv[3]) != 32 ||
        strspn(argv[3], "0123456789abcdef") != 32) return 64;
    alarm(180);
    signal(SIGPIPE, SIG_IGN);
    struct sockaddr_in address = {.sin_family=AF_INET, .sin_port=htons((unsigned short)port)};
    address.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    int fd = socket(AF_INET, SOCK_STREAM, 0);
    if (fd < 0) { perror("socket"); return 1; }
    if (!strcmp(argv[1], "serve")) {
        if (bind(fd, (struct sockaddr *)&address, sizeof address) || listen(fd, 8)) {
            perror("listen"); return 1;
        }
        printf("READY 127.0.0.1:%ld %s\n", port, argv[3]); fflush(stdout);
        for (int i = 0; i < 32; i++) {
            int peer = accept(fd, NULL, NULL);
            if (peer < 0) { if (errno == EINTR) continue; perror("accept"); return 1; }
            char proof[34]; snprintf(proof, sizeof proof, "%s\n", argv[3]);
            if (write(peer, proof, 33) != 33) { perror("write"); close(peer); return 1; }
            close(peer);
        }
    } else {
        struct timeval timeout = {.tv_sec=3};
        setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof timeout);
        setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &timeout, sizeof timeout);
        if (connect(fd, (struct sockaddr *)&address, sizeof address)) { perror("connect"); return 1; }
        char proof[34] = {0}; size_t used = 0;
        while (used < 33) {
            ssize_t count = read(fd, proof + used, 33 - used);
            if (count <= 0) { perror("read"); return 1; }
            used += (size_t)count;
        }
        if (memcmp(proof, argv[3], 32) || proof[32] != '\n') return 1;
        fputs(proof, stdout);
    }
    close(fd); return 0;
}
