# Premium email

Premium members can request a free `name@student.mitch.pro` address.

- **Request.** `/premium-email/` shows the form (3 to 20 lowercase letters or numbers; reserved
  names such as `admin` are refused). `GET /api/premium/email/status` returns what the page
  needs, `POST /api/premium/email/register` records the request. One live (pending or
  approved) request per member, and an address can be held by one request at a time. A denied
  member may apply again.
- **Review.** Admins use `/staff-dashboard/premium-email/` (`GET /api/admin/premium-email`,
  `POST /api/admin/premium-email/decide`). **Creating the mailbox is manual**: the admin sets it up
  on the mail host first, then approves. Approving or denying emails the member.
- **Popup.** `webserver/premium-email-prompt.js` (loaded by `app-shell.js`) tells Premium members
  without a request that the address exists. It is skipped on the sign-in, application and
  staff pages, and a tab checks at most once per session. Pressing **Dismiss** calls
  `POST /api/premium/email/dismiss`, saved on the account in `premium_email_prompt_dismissed`, so it
  never returns on any device. It also stays away once a request exists or was denied
  (`mitch_lib::premium_email::should_prompt`).

Tables: `premium_email_requests`, `premium_email_prompt_dismissed`. Both are removed by account
deletion (`docs/account-deletion.md`).
