use clap::Parser;
use eyre::Result;
use std::path::PathBuf;
use std::sync::Arc;

// Import from your index-maker project
use index_maker::solver::mint_invoice_manager::MintInvoiceManager;
use symm_core::core::persistence::util::JsonFilePersistence;

/// Migrate MintInvoiceManager from old format to new composite persistence format
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to the InvoiceManager.json file
    #[arg(short, long)]
    file: PathBuf,

    /// Dry run - don't write any files
    #[arg(short, long, default_value_t = false)]
    dry_run: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    println!("=== MintInvoiceManager Migration Tool ===\n");
    println!("File: {:?}", args.file);
    
    if args.dry_run {
        println!("Mode: DRY RUN (no files will be written)\n");
    } else {
        println!("Mode: LIVE (files will be written)\n");
    }

    // Check if file exists
    if !args.file.exists() {
        eprintln!("Error: File not found: {:?}", args.file);
        std::process::exit(1);
    }

    // Read the file
    println!("Reading file...");
    let file_content = std::fs::read_to_string(&args.file)?;
    let root_value: serde_json::Value = serde_json::from_str(&file_content)?;

    // Check if already migrated
    if root_value.get("metadata").is_some() {
        println!("✓ File is already in new format (has 'metadata' key).");
        println!("✓ Nothing to do.");
        return Ok(());
    }

    // Parse old format
    let invoices_array = match root_value.get("invoices") {
        Some(inv) => inv,
        None => {
            eprintln!("Error: No 'invoices' key found. Unknown format.");
            std::process::exit(1);
        }
    };

    use index_maker::solver::mint_invoice::MintInvoice;
    use symm_core::core::bits::Address;
    
    let old_invoices: Vec<(u32, Address, MintInvoice)> = 
        serde_json::from_value(invoices_array.clone())?;

    println!("Found {} invoices in old format\n", old_invoices.len());

    if args.dry_run {
        // Just show what would happen
        for (i, (chain_id, address, invoice)) in old_invoices.iter().enumerate() {
            println!("[{}/{}] Would process: {}", 
                i + 1, 
                old_invoices.len(), 
                invoice.client_order_id
            );
            println!("  -> Chain: {}, Address: {}", chain_id, address);
        }
        
        println!("\n Run without --dry-run to apply changes");
        return Ok(());
    }

    // Create backup
    let backup_path = args.file.with_extension("json.backup");
    println!("Creating backup: {:?}", backup_path);
    std::fs::copy(&args.file, &backup_path)?;
    println!("✓ Backup created\n");

    // Use MintInvoiceManager to do the migration
    // This automatically uses the correct key format and child path logic
    let persistence = Arc::new(JsonFilePersistence::new(&args.file));
    let mut manager = MintInvoiceManager::new(persistence);

    // Add each invoice - this creates child files automatically
    for (i, (chain_id, address, invoice)) in old_invoices.into_iter().enumerate() {
        // This will:
        // 1. Create child file with full invoice data
        // 2. Add metadata entry
        // 3. Update root file
        manager.add_invoice(chain_id, address, invoice)?;
        
        println!("  Done");
    }

    println!();
    println!("=== Migration Summary ===");
    println!(" Updated root file with metadata");
    println!(" Backup saved as: {:?}", backup_path);
    println!("\n Migration complete!");

    Ok(())
}