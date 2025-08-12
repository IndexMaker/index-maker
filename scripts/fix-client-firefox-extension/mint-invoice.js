const toggleDetails = (symbol) => {
    const detailedRows = document.querySelectorAll(`.detailed-row-${symbol}`);
    const toggleIcon = document.getElementById(`toggle-icon-${symbol}`);
    if (detailedRows.length > 0 && toggleIcon) {
        const isHidden = detailedRows[0].style.display === 'none' || detailedRows[0].style.display === '';
        detailedRows.forEach(row => {
            row.style.display = isHidden ? 'table-row' : 'none';
        });
        toggleIcon.textContent = isHidden ? '−' : '+';
    }
};

const lotsTable = document.getElementById('lots-table');
if (lotsTable) {
    lotsTable.addEventListener('click', (event) => {
        let target = event.target;
        while (target && !target.classList.contains('summary-row')) {
            target = target.parentElement;
        }
        if (target && target.classList.contains('summary-row')) {
            const symbol = target.dataset.symbol;
            toggleDetails(symbol);
        }
    });
}

document.addEventListener('DOMContentLoaded', () => {
  const table = document.getElementById('collateral-lots-table');
  if (table) {
    table.addEventListener('click', (event) => {
      const summaryRow = event.target.closest('.summary-row');
      if (summaryRow) {
        const lotId = summaryRow.dataset.lotId;
        const detailedRows = document.querySelectorAll(`.detailed-row-${lotId}`);
        const toggleIcon = document.getElementById(`toggle-icon-${lotId}`);
        
        const isVisible = detailedRows[0].style.display !== 'none';
        
        detailedRows.forEach(row => {
          row.style.display = isVisible ? 'none' : '';
        });

        toggleIcon.textContent = isVisible ? '+' : '-';
      }
    });
  }
});