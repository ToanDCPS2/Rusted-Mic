/* See win32cond.h. */
#include "win32cond.h"

int win32_cond_init(win32_cond_t* cv)
{
  cv->waiters = 0;
  cv->sema = CreateSemaphore(NULL, 0, 0x7fffffff, NULL);
  return cv->sema ? 0 : -1;
}

int win32_cond_destroy(win32_cond_t* cv)
{
  if (cv->sema) CloseHandle(cv->sema);
  cv->sema = NULL;
  return 0;
}

int win32_cond_wait(win32_cond_t* cv, HANDLE* mutex)
{
  InterlockedIncrement(&cv->waiters);
  ReleaseMutex(*mutex);
  WaitForSingleObject(cv->sema, INFINITE);
  WaitForSingleObject(*mutex, INFINITE);
  return 0;
}

int win32_cond_signal(win32_cond_t* cv)
{
  LONG n = cv->waiters;
  while (n > 0) {
    LONG seen = InterlockedCompareExchange(&cv->waiters, n - 1, n);
    if (seen == n) {
      ReleaseSemaphore(cv->sema, 1, NULL);
      break;
    }
    n = seen;
  }
  return 0;
}

int win32_cond_broadcast(win32_cond_t* cv)
{
  LONG n = InterlockedExchange(&cv->waiters, 0);
  if (n > 0) ReleaseSemaphore(cv->sema, n, NULL);
  return 0;
}
