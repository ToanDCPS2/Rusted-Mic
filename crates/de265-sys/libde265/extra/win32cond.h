/*
 * Condition variables for libde265's Win32 threads, where a mutex is a
 * HANDLE. Written for de265-sys (upstream ships its own extra/win32cond.c,
 * which is missing from this copy): a counting semaphore releases waiters,
 * so a wake-up sent between unlocking and waiting is never lost.
 */
#ifndef DE265_WIN32COND_H
#define DE265_WIN32COND_H

#include <windows.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct {
  HANDLE sema;
  volatile LONG waiters;
} win32_cond_t;

int win32_cond_init(win32_cond_t* cv);
int win32_cond_destroy(win32_cond_t* cv);
int win32_cond_wait(win32_cond_t* cv, HANDLE* mutex);
int win32_cond_signal(win32_cond_t* cv);
int win32_cond_broadcast(win32_cond_t* cv);

#ifdef __cplusplus
}
#endif

#endif
