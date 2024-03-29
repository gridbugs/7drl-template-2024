spatial_table::declare_layers_module! {
    layers {
        floor: Floor,
        feature: Feature,
        character: Character,
        item: Item,
    }
}
pub use layers::{Layer, LayerTable, Layers};
pub type SpatialTable = spatial_table::SpatialTable<Layers>;
pub type Location = spatial_table::Location<Layer>;
