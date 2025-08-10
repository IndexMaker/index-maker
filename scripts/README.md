# Scripts

## Firefox Extension

We deceloped Firefox extension as this allows for better organisation of app
files for local web app without the need to run local web server.

This extension opens *FIX Test Client* web app in the new tab.

### Developer Mode

1. Download *Firefox Developer Edition*
1. Navigate to `about:debugging#/runtime/this-firefox`
1. Click `Load Temporary Add-on...` button
1. Select `manifest.json` file from `fix-client-firefox-extension`
1. Click `Extensions` button to open the extension
1. New tab will open with *FIX Test Client* app

### Install Extension

1. Download *Firefox Developer Edition*
1. Navigate to `about:config`
1. Click `Accept the Rist and Continue` button
1. Type `xpinstall.signatures.required` and change it to `false`
1. Zip only the contents without the directory of the `fix-client-firefox-extension`
1. Navigate to `about:addons`
1. Click *gear* icon button
1. Select `Install Add-on From File...` menu item
1. Confirm when Firefox asks about installing unverified extension
1. Click `Extensions` button to open the extension
1. New tab will open with *FIX Test Client* app

