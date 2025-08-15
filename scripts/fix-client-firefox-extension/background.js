// File: background.js
// This script listens for the browser action click and opens the UI in a new tab.

browser.browserAction.onClicked.addListener(() => {
  browser.tabs.create({
    url: browser.extension.getURL("tab.html")
  });
});