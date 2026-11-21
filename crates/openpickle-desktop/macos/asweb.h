#pragma once
#ifdef __cplusplus
extern "C" {
#endif
/* callback: url non-NULL on success; error non-NULL on failure/cancel. Either may be NULL. */
typedef void (*openpickle_asweb_cb)(const char *url, const char *error, void *ctx);
/* Starts ASWebAuthenticationSession on the main queue. Returns 0 on start OK, nonzero on failure.
   On zero return, calls cb exactly once, including explicit cancellation.
   On nonzero return, never calls cb; the caller still owns ctx.
   Holds the session until completion or openpickle_asweb_cancel. */
int openpickle_asweb_start(const char *url, const char *callback_scheme, openpickle_asweb_cb cb, void *ctx);
void openpickle_asweb_cancel(void);
#ifdef __cplusplus
}
#endif
