// This file is chart-setup.js
// It must be a separate, static file in your extension.

// The `window` object of the newly opened window will contain our data.
const dataLabels = window.chartDataLabels;
const dataValues = window.chartDataValues;

// Ensure both datasets exist before trying to create the chart
if (dataLabels && dataValues) {
    const ctx = document.getElementById('assetValuesChart');

    if (ctx) {
        new Chart(ctx, {
            type: 'bar',
            data: {
                labels: dataLabels,
                datasets: [{
                    label: 'Asset Value',
                    data: dataValues,
                    backgroundColor: 'rgba(54, 162, 235, 0.6)',
                    borderColor: 'rgb(54, 162, 235)',
                    borderWidth: 1
                }]
            },
            options: {
                responsive: true,
                scales: {
                    y: {
                        beginAtZero: true
                    },
                    x: {
                        ticks: {
                            // NEW: Disable automatic label skipping
                            autoSkip: false, 
                            // NEW: Rotate labels by 90 degrees to fit them all
                            maxRotation: 90,
                            minRotation: 90 
                        }
                    }
                }
            }
        });
    }
}