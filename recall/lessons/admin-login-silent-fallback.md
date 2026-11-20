# admin-login-silent-fallback
- trigger: admin login with a wrong password showed no error — the page silently used the raw password as a Bearer token, failed auth, and just showed the login again, so the user only saw "login not happening"
- rule: on login failure, always show the real error to the user; never fall through to using a password as a token
- applies: admin.html unlock(), any login flow
