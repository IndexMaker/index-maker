import { toUtf8Bytes } from "https://esm.sh/ethers@6.13.0";
import * as secp from "https://esm.sh/@noble/secp256k1@1.7.1";
import { sha256 } from "https://esm.sh/@noble/hashes@1.3.3/sha256";
import { hmac } from "https://esm.sh/@noble/hashes@1.3.3/hmac";

// Polyfill for secp to ensure compatibility
secp.utils.hmacSha256Sync = (key, msg) => hmac(sha256, key, msg);

// ---
// ## Constants
// ---

const MSG_TYPES = {
  NEW_INDEX_ORDER: "NewIndexOrder",
  CANCEL_INDEX_ORDER: "CancelIndexOrder",
  INDEX_ORDER_FILL: "IndexOrderFill",
  MINT_INVOICE: "MintInvoice",
  NEW_QUOTE_REQUEST: "NewQuoteRequest",
  CANCEL_QUOTE_REQUEST: "CancelQuoteRequest",
  ACK: "ACK",
  NAK: "NAK",
};
const ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";

// ---
// ## DOM Helpers and Logging
// ---

let ws = null;
const $ = (id) => document.getElementById(id);

// Custom alert function to replace window.alert
const customAlert = (message) => {
  const modal = document.getElementById('custom-alert-modal');
  const msgElement = document.getElementById('custom-alert-message');
  const closeBtn = document.getElementById('custom-alert-close');

  if (msgElement && modal && closeBtn) {
    msgElement.textContent = message;
    modal.style.display = 'flex';
    closeBtn.onclick = () => {
      modal.style.display = 'none';
    };
  } else {
    console.error("Alert modal elements not found.");
  }
};


const log = (msg, err = false) => {
  const div = document.createElement("div");
  // Check if the message is the MintInvoice special case
  if (typeof msg === 'object' && msg.msgType === 'MintInvoice') {
    const link = document.createElement('a');
    link.href = '#';
    link.textContent = msg.text;
    link.onclick = (event) => {
      event.preventDefault();
      createInvoicePage(msg.data);
    };
    div.appendChild(document.createTextNode(`[${new Date().toISOString()}] `));
    div.appendChild(link);
  } else {
    div.textContent = `[${new Date().toISOString()}] ${msg}`;
    if (err) div.style.color = "red";
  }
  $("log").appendChild(div);
  $("log").scrollTop = $("log").scrollHeight;
};

// ---
// ## New Function to Create Invoice Page
// ---

// Function to format a date string into YYYY-mm-dd HH:MM:ss.fff format (local time)
const formatDateTime = (isoString) => {
  const date = new Date(isoString);
  const pad = (num) => String(num).padStart(2, '0');
  const padMs = (num) => String(num).padStart(3, '0');

  const year = date.getFullYear();
  const month = pad(date.getMonth() + 1);
  const day = pad(date.getDate());
  const hours = pad(date.getHours());
  const minutes = pad(date.getMinutes());
  const seconds = pad(date.getSeconds());
  const milliseconds = padMs(date.getMilliseconds());

  return `${year}-${month}-${day} ${hours}:${minutes}:${seconds}.${milliseconds}`;
};


const createInvoicePage = (invoiceData) => {
  const newWindow = window.open('about:blank', '_blank');
  if (!newWindow) {
    customAlert("Please allow popups to view the invoice.");
    return;
  }

  // Calculate the summary for each asset symbol
  const assetSummary = invoiceData.lots.reduce((acc, lot) => {
    const { symbol, price, assigned_quantity, assigned_fee } = lot;
    if (!acc[symbol]) {
      acc[symbol] = {
        totalAssignedQty: 0,
        totalAssignedFee: 0,
        totalPriceQty: 0,
        averagePrice: 0,
      };
    }
    // Convert string values to numbers for correct calculations
    const numericPrice = parseFloat(price);
    const numericAssignedQty = parseFloat(assigned_quantity);
    const numericAssignedFee = parseFloat(assigned_fee);

    acc[symbol].totalAssignedQty += numericAssignedQty;
    acc[symbol].totalAssignedFee += numericAssignedFee;
    acc[symbol].totalPriceQty += numericPrice * numericAssignedQty;
    return acc;
  }, {});

  // Finalize calculations for average price
  Object.keys(assetSummary).forEach((symbol) => {
    const summary = assetSummary[symbol];
    summary.averagePrice =
      summary.totalAssignedQty > 0
        ? summary.totalPriceQty / summary.totalAssignedQty
        : 0;
  });

  // Sort the lots array by symbol and then by created_timestamp for the detailed table
  const sortedLots = [...invoiceData.lots].sort((a, b) => {
    if (a.symbol < b.symbol) return -1;
    if (a.symbol > b.symbol) return 1;
    // If symbols are the same, sort by timestamp
    return new Date(a.created_timestamp) - new Date(b.created_timestamp);
  });

  let lotsTableContent = '';
  const uniqueSymbols = [...new Set(sortedLots.map(lot => lot.symbol))];

  const chartDataLabels = [...new Set(invoiceData.lots.map(lot => lot.symbol))].sort();
  const chartDataValues = chartDataLabels.map(symbol => assetSummary[symbol].totalPriceQty);

  newWindow.chartDataLabels = chartDataLabels;
  newWindow.chartDataValues = chartDataValues;

  uniqueSymbols.forEach(symbol => {
    const summary = assetSummary[symbol];
    const lotsForSymbol = sortedLots.filter(lot => lot.symbol === symbol);

    // Build the summary row
    lotsTableContent += `
  <tr class="summary-row" data-symbol="${symbol}">
    <td class="table-cell icon-cell"><span id="toggle-icon-${symbol}">+</span></td>
    <td class="table-cell">${symbol}</td>
    <td class="table-cell right">${summary.totalPriceQty.toFixed(7)}</td>
    <td class="table-cell right">~ ${summary.averagePrice.toFixed(7)}</td>
    <td class="table-cell right">${summary.totalAssignedQty.toFixed(7)}</td>
    <td class="table-cell right">${summary.totalAssignedFee.toFixed(7)}</td>
    <td class="table-cell" colspan="5"></td>
  </tr>
`;

    // Build the detailed rows, hidden by default
    lotsForSymbol.forEach(lot => {
      lotsTableContent += `
    <tr class="table-row detailed-row-${symbol}" style="display: none;">
      <td class="table-cell"></td>
      <td class="table-cell">${lot.symbol}</td>
      <td class="table-cell right">${parseFloat(lot.price * lot.assigned_quantity).toFixed(7)}</td>
      <td class="table-cell right">${parseFloat(lot.price).toFixed(7)}</td>
      <td class="table-cell right">${parseFloat(lot.assigned_quantity).toFixed(7)}</td>
      <td class="table-cell right">${parseFloat(lot.assigned_fee).toFixed(7)}</td>
      <td class="table-cell right">${formatDateTime(lot.assigned_timestamp)}</td>
      <td class="table-cell border-left">${lot.lot_id}</td>
      <td class="table-cell right">${parseFloat(lot.original_quantity).toFixed(7)}</td>
      <td class="table-cell right">${parseFloat(lot.original_fee).toFixed(7)}</td>
      <td class="table-cell right">${formatDateTime(lot.created_timestamp)}</td>
    </tr>
  `;
    });
  });

  const headerHtml = `
<div class="title">
  <h1>Invoice Details</h1>
</div>
<div class="content-container">
  <p><strong>Chain ID:</strong> ${invoiceData.chain_id}</p>
  <p><strong>Address:</strong> ${invoiceData.address}</p>
  <p><strong>Client Order ID:</strong> ${invoiceData.client_order_id}</p>
  <p><strong>Payment ID:</strong> ${invoiceData.payment_id}</p>
  <p><strong>Symbol:</strong> ${invoiceData.symbol}</p>
</div>
`;

  const chartHtml = `
<div class="content-container">
  <h2>Chart</h2>
  <div style="width: 80%; margin: 20px auto;">
    <canvas id="assetValuesChart"></canvas>
  </div>
  <script src="lib/chart.umd.js"></script>
  <script src="chart-setup.js"></script>
</div>
`;

  const accountingSheetHtml = `
<div class="content-container">
  <h2>Accounting Summary</h2>
  <table class="data-table accounting-container">
    <thead>
      <tr>
        <th class="table-header left">Description</th>
        <th class="table-header right">Amount</th>
      </tr>
    </thead>
    <tbody>
      <tr class="table-row">
        <td class="table-cell left">Fill Rate</td>
        <td class="table-cell right">${parseFloat(invoiceData.fill_rate).toFixed(7)}</td>
      </tr>
      <tr class="table-row">
        <td class="table-cell left">Assets Value</td>
        <td class="table-cell right">${parseFloat(invoiceData.assets_value).toFixed(7)}</td>
      </tr>
      <tr class="table-row">
        <td class="table-cell left">Exchange Fee</td>
        <td class="table-cell right">${parseFloat(invoiceData.exchange_fee).toFixed(7)}</td>
      </tr>
      <tr class="table-row">
        <td class="table-cell left">Management Fee</td>
        <td class="table-cell right">${parseFloat(invoiceData.management_fee).toFixed(7)}</td>
      </tr>
      <tr class="table-row">
        <td class="table-cell left">Total Amount</td>
        <td class="table-cell right">${parseFloat(invoiceData.total_amount).toFixed(7)}</td>
      </tr>
      <tr class="table-row">
        <td class="table-cell left">Amount Paid</td>
        <td class="table-cell right">${parseFloat(invoiceData.amount_paid).toFixed(7)}</td>
      </tr>
      <tr class="table-row">
        <td class="table-cell left">Amount Remaining</td>
        <td class="table-cell right">${parseFloat(invoiceData.amount_remaining).toFixed(7)}</td>
      </tr>
    </tbody>
  </table>
</div>
`;

  const lotsTableHtml = `
<div class="content-container">
  <h2>Asset Lots Details</h2>
  <table class="data-table" id="lots-table">
    <colgroup>
      <col style="width: 20px;">
      <col style="width: 10%;">
      <col style="width: 10%;">
      <col style="width: 15%;">
      <col style="width: 10%;">
      <col style="width: 10%;">
      <col style="width: 10%;">
      <col style="width: 15%;">
      <col style="width: 10%;">
      <col style="width: 10%;">
      <col style="width: 10%;">
    </colgroup>
    <thead>
      <tr>
        <th class="table-header left" style="width: 20px;"></th>
        <th class="table-header left">Symbol</th>
        <th class="table-header right">Value</th>
        <th class="table-header right">Price</th>
        <th class="table-header right">Assigned Qty</th>
        <th class="table-header right">Assigned Fee</th>
        <th class="table-header right">Assigned At</th>
        <th class="table-header left border-left">Lot ID</th>
        <th class="table-header right">Original Qty</th>
        <th class="table-header right">Original Fee</th>
        <th class="table-header right">Created At</th>
      </tr>
    </thead>
    <tbody>
      ${lotsTableContent}
    </tbody>
  </table>
  <script src="mint-invoice.js"></script>
</div>
`;

  const htmlContent = `
<!DOCTYPE html>
<html lang="en">
<head>
  <title>Mint Invoice</title>
  <link rel="stylesheet" href="mint-invoice.css">
</head>
<body>
  ${headerHtml}
  ${accountingSheetHtml}
  ${chartHtml}
  ${lotsTableHtml}
</body>
</html>
`;
  newWindow.document.write(htmlContent);
  newWindow.document.close();
};

// ---
// ## Helper Function
// ---

const generateClientId = (timestamp, address, chainId, seqNum) => {
  // Combine unique message data to create a deterministic hash
  const combinedData = `${timestamp}${address}${chainId}${seqNum}`;
  const hash = sha256(toUtf8Bytes(combinedData));

  const hashHex = secp.utils.bytesToHex(hash);

  // Derive the three-part alphabetic prefix
  const code1 = `${ALPHABET[parseInt(hashHex.slice(0, 2), 16) % 26]}${ALPHABET[parseInt(hashHex.slice(2, 4), 16) % 26]}${ALPHABET[parseInt(hashHex.slice(4, 6), 16) % 26]}`;
  const code2 = `${ALPHABET[parseInt(hashHex.slice(6, 8), 16) % 26]}${ALPHABET[parseInt(hashHex.slice(8, 10), 16) % 26]}${ALPHABET[parseInt(hashHex.slice(10, 12), 16) % 26]}`;
  const code3 = `${ALPHABET[parseInt(hashHex.slice(12, 14), 16) % 26]}${ALPHABET[parseInt(hashHex.slice(14, 16), 16) % 26]}${ALPHABET[parseInt(hashHex.slice(16, 18), 16) % 26]}`;

  // Derive a four-digit numeric suffix
  const numSuffix = (parseInt(hashHex.slice(18, 22), 16) % 9000) + 1001;

  return `${code1}-${code2}-${code3}-${numSuffix}`;
};

const getMinimalSignPayload = (msg) => {
  const { msg_type } = msg.standard_header;
  const idKey = msg_type.includes("Quote") ? "client_quote_id" : "client_order_id";

  if (
    [
      MSG_TYPES.NEW_INDEX_ORDER,
      MSG_TYPES.CANCEL_INDEX_ORDER,
      MSG_TYPES.NEW_QUOTE_REQUEST,
      MSG_TYPES.CANCEL_QUOTE_REQUEST,
      MSG_TYPES.INDEX_ORDER_FILL,
      MSG_TYPES.MINT_INVOICE,
    ].includes(msg_type)
  ) {
    return {
      msg_type,
      id: msg[idKey],
    };
  }
  return null;
};

// ---
// ## Quote Management
// ---

const pendingQuotes = {};

const addQuote = (msg) => {
  pendingQuotes[msg.client_quote_id] = msg;
}

const updateQuote = (msg) => {
  const quotesBody = document.getElementById('quotes');

  if (msg.standard_header.msg_type === 'IndexQuoteResponse') {
    const clientQuoteId = msg.client_quote_id;
    const originalQuote = pendingQuotes[clientQuoteId];
    const amount = originalQuote ? originalQuote.amount : 'N/A';
    const quantityPossible = parseFloat(msg.quantity_possible);

    const newRow = quotesBody.insertRow(-1);
    newRow.id = `quote-${clientQuoteId}`;
    newRow.className = 'quote-row';

    newRow.innerHTML = `
    <td class="client-quote-id" style="text-align: left">${clientQuoteId}</td>
    <td style="text-align: right">${amount}</td>
    <td style="text-align: right">${quantityPossible.toFixed(7)}</td>
    <td style="text-align: right">${formatDateTime(msg.standard_header.timestamp)}</td>
  `;

  }
};

// ---
// ## Order Management
// ---

const pendingOrders = {};

const addOrder = (msg) => {
  pendingOrders[msg.client_order_id] = msg;
}

const updateOrder = (msg) => {
  const ordersBody = document.getElementById('orders');

  if (msg.standard_header.msg_type === 'NewIndexOrder' && msg.status === 'new') {
    // 1. If it's a new order message, add a new table row.
    const clientOrderId = msg.client_order_id;
    const originalOrder = pendingOrders[clientOrderId];
    const amount = originalOrder ? originalOrder.amount : 'N/A';

    const newRow = ordersBody.insertRow(-1); // Insert at the end of the body
    newRow.id = `order-${clientOrderId}`;
    newRow.className = 'order-row';

    newRow.innerHTML = `
    <td class="client-order-id" style="text-align: left">${clientOrderId}</td>
    <td style="text-align: right">${amount}</td>
    <td>
      <div class="progress-bar-container">
        <div class="progress-bar" style="width: 0%;"></div>
      </div>
    </td>
    <td class="progress-pct" style="text-align: right">0</td>
    <td style="text-align: right">${formatDateTime(msg.standard_header.timestamp)}</td>
  `;

  } else if (msg.standard_header.msg_type === 'IndexOrderFill') {
    // 2. If it's a fill message, update the progress of an existing row.
    const clientOrderId = msg.client_order_id;
    const fillRate = Math.min(parseFloat(msg.fill_rate) * 100, 100.0);
    const orderRow = document.getElementById(`order-${clientOrderId}`);

    if (orderRow) {
      const progressBar = orderRow.querySelector('.progress-bar');
      if (progressBar) {
        progressBar.style.width = `${fillRate}%`;
      }
      const progressPct = orderRow.querySelector('.progress-pct');
      if (progressPct) {
        progressPct.innerHTML = `${fillRate.toFixed(1)}`;
      }
    }
  } else if (msg.standard_header.msg_type === 'MintInvoice') {
    // 3. If it's a MintInvoice message, make the Client Order ID clickable.
    const clientOrderId = msg.client_order_id;
    const orderRow = document.getElementById(`order-${clientOrderId}`);

    if (orderRow) {
      const clientIdCell = orderRow.querySelector('.client-order-id');
      if (clientIdCell) {
        // Add a class and a click event listener to the table cell
        clientIdCell.classList.add('clickable-id');
        clientIdCell.addEventListener('click', () => {
          // NOTE: The createInvoicePage function must be defined elsewhere in your code.
          createInvoicePage(msg);
        });
      }
    }
  }
};

// ---
// ## Event Handlers
// ---

document.addEventListener("DOMContentLoaded", () => {
  const modeSelect = $("mode");
  const privateKeySection = $("privateKeySection");
  const wsUrlInput = $("wsUrl");
  const connectBtn = $("connectBtn");
  const disconnectBtn = $("disconnectBtn");
  const signBtn = $("signBtn");
  const fixMessageJsonTextarea = $("fixMessageJson");
  const logDiv = $("log");
  const quotesLabel = $("quotesLabel");
  const ordersLabel = $("ordersLabel");
  const quotesBody = $("quotesBody");
  const ordersBody = $("ordersBody");

  modeSelect.onchange = () => {
    const mode = modeSelect.value;
    if (mode === "index") {
      privateKeySection.style.display = "block";
      quotesLabel.style.display = "none";
      ordersLabel.style.display = "block";
      quotesBody.style.display = "none";
      ordersBody.style.display = "block";
    } else {
      privateKeySection.style.display = "none";
      quotesLabel.style.display = "block";
      ordersLabel.style.display = "none";
      quotesBody.style.display = "block";
      ordersBody.style.display = "none";
    }

    const baseMessage = {
      standard_header: {
        sender_comp_id: "CLIENT",
        target_comp_id: "SERVER",
        seq_num: 1,
        timestamp: new Date().toISOString(),
      },
      chain_id: 1,
      address: "",
      symbol: "SY100",
      side: "buy",
      amount: "5",
    };

    const client_order_id = generateClientId(
      baseMessage.standard_header.timestamp,
      baseMessage.address,
      baseMessage.chain_id,
      baseMessage.standard_header.seq_num
    );

    const message =
      mode === "index"
        ? {
          ...baseMessage,
          standard_header: { ...baseMessage.standard_header, msg_type: MSG_TYPES.NEW_INDEX_ORDER },
          client_order_id: client_order_id,
        }
        : {
          ...baseMessage,
          standard_header: { ...baseMessage.standard_header, msg_type: MSG_TYPES.NEW_QUOTE_REQUEST },
          client_quote_id: client_order_id,
        };

    fixMessageJsonTextarea.value = JSON.stringify(message, null, 2);
  };

  connectBtn.onclick = () => {
    const url = wsUrlInput.value;
    ws = new WebSocket(url);
    ws.onopen = () => {
      log("WebSocket connected");
      connectBtn.disabled = true;
      disconnectBtn.disabled = false;
    };
    ws.onclose = () => {
      log("WebSocket disconnected");
      connectBtn.disabled = false;
      disconnectBtn.disabled = true;
    };
    ws.onerror = () => log("WebSocket error", true);
    ws.onmessage = (e) => {
      const msg = JSON.parse(e.data);

      const { msg_type } = msg.standard_header || {};

      if (msg_type === MSG_TYPES.MINT_INVOICE) {
        log({ msgType: 'MintInvoice', text: `Received MintInvoice for payment_id: ${msg.payment_id}`, data: msg });
        updateOrder(msg);
        return;
      }

      log("Received:\n" + JSON.stringify(msg, null, 2));

      if (msg_type === MSG_TYPES.ACK || msg_type === MSG_TYPES.NAK) {
        log("ℹ️ Received ACK — updating seq_num, client_order_id, and timestamp.");
        const currentMessage = JSON.parse(fixMessageJsonTextarea.value);

        if (currentMessage.standard_header) {
          // Increment seq_num
          if (currentMessage.standard_header.seq_num) {
            currentMessage.standard_header.seq_num = msg.ref_seq_num + 1;
          }

          // Update timestamp
          const now = new Date();
          now.setSeconds(now.getSeconds() + 30);
          now.setMilliseconds(0);
          currentMessage.standard_header.timestamp = now.toISOString();

          // Generate a new, hash-based client_order_id
          if (currentMessage.client_order_id) {
            currentMessage.client_order_id = generateClientId(
              currentMessage.standard_header.timestamp,
              currentMessage.address,
              currentMessage.chain_id,
              currentMessage.standard_header.seq_num
            );
          }
          if (currentMessage.client_quote_id) {
            currentMessage.client_quote_id = generateClientId(
              currentMessage.standard_header.timestamp,
              currentMessage.address,
              currentMessage.chain_id,
              currentMessage.standard_header.seq_num
            );
          }
        }

        fixMessageJsonTextarea.value = JSON.stringify(currentMessage, null, 2);
      }

      if (msg_type?.includes("Quote") || msg_type === MSG_TYPES.ACK || msg_type === MSG_TYPES.NAK) {
        log("ℹ️ Skipping signature verification for message type: " + msg_type);
        updateQuote(msg);
        return;
      }

      const { standard_trailer } = msg;
      if (standard_trailer?.signature && standard_trailer?.public_key) {
        const { signature, public_key } = standard_trailer;
        const signPayload = getMinimalSignPayload(msg);

        if (!signPayload) {
          log("Unsupported msg_type for signature verification", true);
          return;
        }

        const hash = sha256(toUtf8Bytes(JSON.stringify(signPayload)));
        const sigBytes = secp.utils.hexToBytes(signature[0].slice(2));
        const pubKeyBytes = secp.utils.hexToBytes(public_key[0].slice(2));

        const verified = secp.verify(sigBytes, hash, pubKeyBytes);
        log(verified ? "✅ Signature verified!" : "❌ Signature verification failed!", !verified);

        updateOrder(msg);
      }
    };
  };

  disconnectBtn.onclick = () => {
    if (ws) ws.close();
  };

  signBtn.onclick = async () => {
    try {
      const rawText = fixMessageJsonTextarea.value.trim();
      const msgObj = JSON.parse(rawText);
      const mode = modeSelect.value;

      if (mode === "index") {
        const privateKeyHex = $("privateKey").value.trim().slice(2);
        if (!privateKeyHex) {
          return log("Private key is required", true);
        }

        addOrder(msgObj);

        const privateKey = secp.utils.hexToBytes(privateKeyHex);
        const signPayload = getMinimalSignPayload(msgObj);
        if (!signPayload) {
          throw new Error("Unsupported msg_type for signing");
        }

        const hash = sha256(toUtf8Bytes(JSON.stringify(signPayload)));
        const sig = secp.signSync(hash, privateKey, { canonical: true, der: false });
        const signatureHex = "0x" + secp.utils.bytesToHex(sig.slice(0, 64));
        const pubKey = secp.getPublicKey(privateKey, false);
        const pubKeyHex = "0x" + secp.utils.bytesToHex(pubKey);

        msgObj.standard_trailer = {
          public_key: [pubKeyHex],
          signature: [signatureHex],
        };
      } else {
        delete msgObj.standard_trailer;
        addQuote(msgObj);
      }

      log("Sending:\n" + JSON.stringify(msgObj, null, 2));

      if (ws) {
        ws.send(JSON.stringify(msgObj));
      } else {
        log("WebSocket is not connected. Please connect first.", true);
      }
    } catch (e) {
      log("Error: " + e.message, true);
    }
  };
});