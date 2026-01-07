---
description: Update Notion LP Tracker after sending an email
argument-hint: [lp_name (optional)]
---

Update my Notion LP Tracker for the LP I just emailed. If an LP name is provided ("$ARGUMENTS"), use that. Otherwise, infer the LP from the email I just sent in the conversation above.

1. Search for the LP in my Notion LP Tracker database
2. Update the following properties:
   - **Last Contact Date**: today's date
   - **Next Action Date**: one week from today
   - **Next Steps**: "Follow up on email sent [today's date]"
   - **Last Contact Summary**: Write a one-sentence summary of the email I just sent to this LP (reference the conversation context above)

Use the expanded Notion date property format:
- `date:Last Contact Date:start` and `date:Last Contact Date:is_datetime`
- `date:Next Action Date:start` and `date:Next Action Date:is_datetime`
