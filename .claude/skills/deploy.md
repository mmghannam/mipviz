---
name: deploy
description: Build and deploy mipviz to the Raspberry Pi
user_invocable: true
---

Deploy mipviz to the Raspberry Pi (ssh pi). Steps:

1. Sync the project files (excluding target/ and .git/):
   ```
   rsync -avz --exclude target --exclude .git /Users/ghannam/projects/instance-investigator/ pi:~/instance-investigator/
   ```

2. Build the release binary on the Pi:
   ```
   ssh pi "cd ~/instance-investigator && cargo build --release"
   ```

3. Restart the systemd service:
   ```
   ssh pi "sudo systemctl restart mipviz"
   ```

4. Verify it's running:
   ```
   ssh pi "systemctl status mipviz --no-pager"
   ```

5. Report: "Deployed to https://mipviz.mghannam.com"
