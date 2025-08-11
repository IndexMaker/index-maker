# Scripts

## Firefox Extension

We deceloped Firefox extension as this allows for better organisation of app
files for local web app without the need to run local web server.

This extension opens *FIX Test Client* web app in the new tab.

## Loom Videos

Checkout my recordings on Loom 🎥

- [Setting up Firefox extension and sending Index Quotes](https://www.loom.com/share/37c18d12627842de8adbba0a9581442b?sid=dfdeacb2-d933-4737-8bb1-c1f0a120c16a)

- [Sending Index Orders](https://www.loom.com/share/adb2eb32ffc347e39e63c65bb44717b7?sid=262c8f74-637b-4b60-9dfe-459554ce6bab)

- [Mint Invoices](https://www.loom.com/share/ddd229284d5249b8be49f08a9148985b?sid=82d77e8e-e67d-46cd-84c7-d2a3d325e90b)

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

