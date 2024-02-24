fn wrap(variables) {
    (variables.prefix ?? this) + " << " + this + " >> " + (variables.suffix ?? this)
}
